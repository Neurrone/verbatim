# Security model

Proposed on 2026-10-31, pending Dickson's decisions in section 9. This document sets out who Verbatim must defend against, what it protects, the rules that follow, and how far today's code and the planned milestones meet them. It is a model, not a build plan: each rule names the milestone where its work belongs, and every build that takes one up gets the usual design and adversarial review.

The working principle is to remove attack surface by architecture or by product choice first, and to add checks only where no structural answer exists. A check guards one path; a surface that does not exist cannot be attacked by any path.

## 1. Decisions required

Section 9 lists the questions in full. In short:

1. Split the control plane into a narrow user endpoint that is always on and a developer endpoint that is off unless developer mode is on, and decide whether signed builds with UIAccess carry the developer endpoint at all.
2. Ignore Verbatim gestures in keys that a lower-integrity process injected, while still accepting keys injected at the same level, which NVDA accepts and users' scripts rely on.
3. Make automatic crash dumps content-free by default, and write content only into a dump the user asks for.
4. Move per-user state (settings, logs and dumps) out of the executable's folder for installed copies, as NVDA does, and keep it beside the executable only for portable copies.
5. Switch to eSpeak NG for the rest of the session after a synthesizer crashes repeatedly within a short window.
6. Watch the extension developer folder only when developer mode is on.

## 2. What is protected

In priority order:

1. **Availability.** A silent or hung screen reader leaves a blind user without a computer, so denial of service counts as a security failure here, not a nuisance. Any application, page or terminal output that can silence Verbatim is an attack on the user.
2. **Privilege boundaries.** Verbatim must never let a less trusted process reach something it could not reach on its own. Once Verbatim has UIAccess (M8.1) it can read elevated windows and send them input, and the secure-desktop instance runs as SYSTEM. Either makes Verbatim a tempting stepping stone to higher privilege.
3. **The user's content.** Verbatim sees everything on screen, every keystroke and the clipboard. What it keeps, where it keeps it and who can read it is the privacy question raised by Windows Recall.
4. **What the user hears.** For a blind user, the speech is the interface. An extension that rewrites what a button says can deceive them in a way a sighted user would notice at once.

## 3. Adversaries and trust boundaries

Each class below says whether a real boundary exists, because effort spent defending a non-boundary is wasted.

### 3.1 Applications being read

Every application Verbatim reads is untrusted input. This includes web pages through the browser, terminal output from an SSH server, documents and chat messages. Content from the network therefore reaches Verbatim's parsers, so the realistic remote attacker is someone who controls text the user reads, not someone who attacks Verbatim directly. The same defences cover buggy applications, which are far more common than malicious ones: cyclic parents, endless sibling chains, huge child counts, multi-megabyte names, event floods and hung providers.

This is a real boundary, and the most important one.

### 3.2 Processes of the same user at the same integrity level

Windows offers no boundary between processes of one user at one integrity level. Such a process can open Verbatim's process with full access, write its memory, load a DLL into it, send it input and read every window through UIA itself. Defending Verbatim's settings, logs or pipe against it gains nothing, because it can do directly whatever it would do through Verbatim.

This answers the question of a program editing Verbatim's configuration:

- **Settings in the user's profile cannot be protected** from that user's programs, and need not be. The rule is that configuration is data and never code (rule C2), so editing it can change how Verbatim behaves but cannot run code or cross a boundary.
- **Files under Program Files are protected already.** Only an administrator can write there, and a non-elevated program cannot. The defence is free as long as Verbatim never makes any folder of its installation writable by the user. Today Verbatim writes `settings.toml`, `logs` and `dumps` beside its executable, so an installed copy must move them to the user's profile. Making the installation folder writable instead would let any program plant a DLL or replace a binary that Windows later starts with UIAccess, or as SYSTEM on the sign-in screen.

The one exception is UIAccess. Windows starts a UIAccess process at a raised integrity level: the caller's level plus 16 for a standard user, and High for an administrator's split token, so programs at the user's ordinary level cannot write its memory. Once Verbatim has UIAccess, everything that steers it (its pipe, injected keys, window messages, configuration and extensions) becomes a way for those programs to borrow its reach into elevated windows. That is why sections 4.4 and 4.6 tighten as M8.1 approaches.

These rules are defence in depth, not a boundary Verbatim can promise. James Forshaw showed in 2019 that an ordinary process can duplicate the token of any running UIAccess process, such as the on-screen keyboard, and start its own UIAccess process, and he called UIAccess "not a security boundary". Microsoft's servicing criteria do not list UAC as a boundary either. So the goal is that Verbatim adds no easier path than the ones Windows already has. M8.1 starts by measuring what an ordinary process can still do to a UIAccess Verbatim on the target Windows build (rule S5).

### 3.3 Lower-trust processes

Processes at Low integrity, AppContainers (browser renderers, Store apps) and sandboxed code are a real boundary below Verbatim. User Interface Privilege Isolation (UIPI), the Windows rule that blocks window messages and injected input from a lower-integrity process to a higher one, already blocks most paths. Verbatim's job is not to open new ones: no window message filter that lets them in, no pipe they can open, and no gesture they can trigger by injecting keys.

### 3.4 Higher-privilege contexts

These are elevated applications on the user's desktop, and the sign-in and UAC screens, where the secure-desktop instance runs as SYSTEM. Here Verbatim is the potential deputy, and the rules in section 4.6 apply. NVDA's published advisories show this is where screen readers are most often attacked (section 8).

### 3.5 Extensions

Wasm modules (M5) and native synthesizer adapters (M6). The threat is a malicious author, a compromised package, or most often a buggy one. Section 9 of [extensions](extensions.md) already holds this threat model; this document adds the paths that model does not cover (rule E3 and rule E4).

### 3.6 Synthesizers

A native engine can crash or hang on particular text, such as Eloquence's known crash strings, and that text comes from untrusted content. Verbatim cannot stop an engine crashing. It can make a crash cost one utterance rather than speech.

### 3.7 The network

Today nothing listens on the network except the test agent. Remote support (M12) will. The test agent is the most serious finding today (finding F1).

## 4. Rules

Each rule states what holds, then why, then where it stands today.

### 4.1 Untrusted accessibility data

- **T1. Every walk over an application's tree has a hop cap and a time budget, and stops when it meets an element it has already visited.** Reason: a parent that names itself or a cycle of parents must cost a bounded amount of work, and must not be spoken as repeated containers. Today: walks are capped at 64 hops for the outpost's and the listener's ancestors and time-boxed by `ENRICHMENT_BUDGET` and the deadlines, but nothing detects a repeat, so a self-parent yields up to 64 identical ancestors. The remote operation's search for a window above the walk (`crates/verbatim-uia-rops/src/focus.rs`, the second loop of the focus program) has no cap of its own and stops only at UIA's instruction limit, after which the read falls back to the classic path. Detect repeats by runtime id for UIA and by window handle for MSAA, because an MSAA object's COM identity does not match across reads (`docs/crates/verbatim-ia2.md`, "Identity").
- **T2. Every list read is capped before it is fetched, never after.** Reason: the provider decides how much it returns, and the cost is paid when it is fetched. Today: the MSAA child list is capped at 10,000 and dumps at 4,096 nodes, but the classic UIA child listing fetches every child before the dump's cap applies (`children_from` in `crates/verbatim-uia/src/client.rs`).
- **T3. Every string from an application is capped where it is read, and a cut is known to the reader.** Reason: a 30 MB name otherwise travels through Core's memory, the speech pipeline and the synthesizer. Today: text reads are cut to 64 KiB with `truncated` set, but Core never uses the flag for speech, so the cut is silent. Names, values and descriptions are bounded only by the 32 MiB frame. Cap each property at the outpost, for example to the text chunk's 64 KiB, and treat a cut as NVDA does once its behaviour is checked in the pinned source.
- **T4. Work in Core grows at most as n log n in the size of any one application input, or the input is capped first.** Reason: Core is shared by every application, has no memory cap, and one costly input silences everything. Today: terminal alignment in `crates/verbatim-text/src/terminal_rows/align.rs` is quadratic in rows and in row width, and `Likeness::of` allocates the number of distinct characters times the row width over 64 words. A wide row of distinct characters printed by a remote program in a terminal can cost Core hundreds of megabytes. Cap the row width the alignment compares exactly, and above the cap compare rows by hash.
- **T5. Core verifies every claim an outpost makes that it can check itself.** Reason: an outpost is only as trustworthy as the application it reads, and in M7 it loads IA2 proxy code and talks to an injected helper. Today this holds well. Node ids are re-stamped with the outpost the pipe belongs to, frames are capped at 32 MiB before they are allocated, the raw section's ranges are checked, and a malformed frame retires the outpost without a panic in Core. A spare's self-reported process id is forwarded unverified; the supervisor holds the real one from the spawn, and should use that one.
- **T6. Every queue that an application can fill has a stated bound and overload policy.** Reason: a flood is the cheapest denial of service. Today: the outpost's intake applies NVDA's limiter, and terminal output waiting in Core is bounded at 10 MB. The per-source cap for background events is not implemented (architecture section 2), and the reducer's completion port and the outposts' outbound queues have no hard count. Add the background flood cap, and give Core a memory cap through a job object of its own so an unforeseen growth ends in a restart (rule A2) rather than paging the machine to a halt.
- **T7. An application can lie, and Verbatim does not try to detect it.** An application can label a Delete button "OK", exactly as it can for sighted users. This is out of scope. Extensions are different, because the user installed them to change what Verbatim says (rule E3).

### 4.2 Availability

- **A1. Recovery is structural.** Today this is Verbatim's strongest property. Per-application outposts (D9) confine a hang or crash in one application's accessibility code to that application. The listener's callbacks make no cross-process call (D13). Every call has a deadline, abandoned workers are replaced, and failing outposts are killed and restarted with backoff. A hostile application therefore costs reading that application, not the screen reader. NVDA, whose one main thread runs everything, cannot offer this.
- **A2. Core restarts itself after a crash, with a breaker.** Reason: Core is the one process whose loss silences everything. Today: a panic writes a flight dump, and then Verbatim is gone. Register with Windows' restart manager (`RegisterApplicationRestart`), or keep a small guardian process, so a crash is followed by speech within seconds, and announce the restart. Stop restarting after a few crashes in a short window, as NVDA's crash handler does, so a crash at startup does not loop.
- **A3. Nothing at lower integrity can end Verbatim.** Today: `allow_quit_across_integrity_levels` in `crates/verbatim-app/src/single_instance.rs` calls `ChangeWindowMessageFilter(WM_QUIT, MSGFLT_ADD)`, which is process-wide, so any Low-integrity process can post `WM_QUIT` to Verbatim's windows and silence it. A replacing instance always runs at the same or a higher level, so remove the call, or use `ChangeWindowMessageFilterEx` on the one hidden window if a case is found that needs it.
- **A4. A synthesizer failure costs one utterance, and a repeated one costs the synthesizer, never speech.** Today: a host that crashes or stays silent for 10 seconds fails the utterance in flight and is started again on the next request, and the utterance is not retried. Falling back to another synthesizer happens only at startup, and nothing caps restarts. So a crash string the user keeps arrowing onto costs a restart each time, and a host that crashes at every start costs silence. Proposed: after three crashes within a minute, switch to eSpeak NG for the session and say why in eSpeak's voice. If eSpeak itself fails, switch to OneCore, as the fallback order already goes at startup. Native adapters for engines with known crash strings filter them in the adapter, where the knowledge belongs. A host that has said nothing for 10 seconds is a long silence, so measure whether a shorter time, counted from the first audio owed, holds without false alarms.

### 4.3 Confidentiality: logs, dumps and the flight recorder

The objection to Recall was a continuous, searchable record of the screen kept on disk by default. A screen reader's flight recorder is a smaller version of the same thing, and has the same failure mode: a store that helps debugging is also a store of passwords, messages and documents.

- **P1. Nothing that carries content is written to disk without the user asking.** Reason: data that is never written cannot leak. Today: the flight recorder (1,024 inputs or 8 MiB, in memory) is written to `dumps` beside the executable on every panic and never pruned, and each dump holds raw reducer inputs, including `CharacterTyped` and the text of `ClipboardCopied`. Proposed: an automatic crash dump is structural. Every string in it is replaced by a placeholder of the same length and character class, so most defects still replay. A dump the user asks for keeps content, and Verbatim says so when it writes one. Accepted cost: a defect that depends on the exact text needs a dump the user asked for.
- **P2. Input from a protected field is masked when it is recorded, not when it is written out.** Reason: the reducer echoes a star for a protected field anyway, so recording a star loses nothing for replay, and the real character never exists anywhere it could leak from. Today: typed characters are recorded unmasked.
- **P3. Logs carry no content at their default level.** Today this nearly holds: no trace call was found that logs utterance text, typed characters or the clipboard, and speech traces carry only an utterance id. Keep it a rule, enforced in review, as M9 adds file logging and the log viewer. Content at a debug level is acceptable only behind an explicit developer setting, and never in the secure-desktop instance.
- **P4. Stored diagnostics have a home, an owner and an end.** Logs and dumps go under the user's local application data (not a roaming folder), readable by that user only, with a cap on count and age: today child logs keep 10 launches with no size cap, and dumps are kept forever. The secure-desktop instance writes no logs or dumps at all, as NVDA's secure mode does.
- **P5. The control plane's speech stream and tree dumps are content.** They are available to any process of the same user, which is no new exposure under section 3.2, but they are a new exposure once UIAccess lets Verbatim read elevated windows. They fall under rule C1.

### 4.4 Control surfaces

- **C1. No control surface is on by default unless a user feature needs it, and each exposes only what that feature needs.** Today: the control pipe starts unconditionally (`crates/verbatim-app/src/main.rs`, "Control plane") and offers `SendKeys`, the speech stream, tree and focus dumps, recorder dumps and `Quit` to any process of the user. Its security descriptor admits the owning user only and refuses remote clients. Its first instance claims the name, so nothing can squat on it. `--secure` does not exist yet. Proposed: two endpoints. A user endpoint is always on and offers only what users' features need: the `.vext` install request from the M5 helper, which opens Verbatim's own confirmation dialog and installs nothing by itself, and status. A developer endpoint, with everything else, exists only while developer mode is on. Neither exists in the secure-desktop instance. Whether signed UIAccess builds carry the developer endpoint at all is question 1.
- **C2. Configuration is data and never code.** Reason: configuration in the user's profile cannot be protected (section 3.2), so it must not be able to name code. Today this holds. The synthesizer is chosen from a fixed set, theme and sound names are restricted to plain names, and no setting names a DLL, an executable or a path. Keep it: a native adapter is found by its name among adapters installed in an administrator-owned folder, never by a path from a setting.
- **C3. Every parser of a file or message has a size limit and fails safe.** Today: `settings.toml`, profiles and themes are read with no size limit. A malformed `settings.toml` makes Verbatim load from `%TEMP%\verbatim-defaults`, a folder any process can write to, and `exe_dir` falls back to the current directory if the executable's path cannot be read. The control pipe reads a line of any length. Proposed: cap each read; on a malformed file, run on compiled-in defaults without reading another folder and say so; refuse to start if the executable's folder is unknown.
- **C4. Development tools are never reachable from the network unauthenticated, and never ship.** Today: `verbatim-agent` binds every interface by default, has no authentication, and can launch any process and read, write or delete any file. The VM provisioning script (`vm/scripts/Initialize-VerbatimHarness.ps1`) opens a firewall port for it. Anyone who can reach that port runs code as the logged-on user (finding F1). Proposed: bind to loopback by default; have the VM harness pass the guest's address explicitly; require a random token created at deploy time and passed by `cargo xtask`; and scope the VM's firewall rule to the host's virtual switch subnet. The agent is not part of any installer.

### 4.5 Input

- **I1. A gesture injected by a lower-integrity process is ignored.** Reason: UIPI already keeps such a process from typing into the user's applications, and Verbatim's gestures must not become a way around that. The keyboard hook can tell from `LLKHF_LOWER_IL_INJECTED`, so this costs nothing on the hook's path. Today: injected keys drive gestures whatever their source (`crates/verbatim-input/src/lib.rs`, the `injected` field), and Verbatim's own `dwExtraInfo` tags are fixed constants, not secrets.
- **I2. Keys injected at the same integrity level keep driving gestures.** Reason: this matches NVDA, users' AutoHotkey scripts rely on it, and such a process has equal power anyway (section 3.2). Once Verbatim has UIAccess and runs above the user's ordinary level, the flag in rule I1 marks ordinary processes' keys as lower too. Verify that on the target build (rule S5), because if it holds, rule I1 alone keeps ordinary programs from driving Verbatim into an elevated window.
- **I3. Verbatim's own injected keys are recognised by the thread and process that sent them, not by a constant tag.** Reason: a fixed tag can be copied by any process to make its keys pass as Verbatim's own. This is low severity until UIAccess.

### 4.6 UIAccess and the secure desktop (preconditions for M8.1)

UIAccess and the SYSTEM instance turn every rule above from good practice into a requirement. M8.1 does not ship until all of these hold:

- **S1. The installation is administrator-owned, signed and contains no writable folder.** Per-user state lives in the profile (rule C2 and rule P4). Every binary is signed, which UIAccess requires anyway.
- **S2. DLL loading is pinned.** Each Verbatim executable calls `SetDefaultDllDirectories` with the system folder and the application folder only, before anything else loads, and links its rarely used system DLLs for delay loading. Reason: a portable copy run from a Downloads folder otherwise loads any DLL planted beside it. Today: no executable restricts its DLL search.
- **S3. The secure-desktop instance reads nothing a user can write.** It reads a configuration copy and the chosen extensions that an elevated step put in an administrator-owned folder ([extensions](extensions.md), section 2.4), runs no control plane, no developer folder, no console and no install path, and writes nothing.
- **S4. No escape hatch on secure screens.** The secure instance never opens a browser, a file or folder dialog, a help file, a URL, the log viewer or any window that can start a process. Reason: about half of NVDA's published advisories are escapes from a secure screen, each through such a hatch: the log viewer, the wx inspection tool, context help, a clipboard error dialog, and a formatting dialog that reached a SYSTEM shell (section 8). Build the secure instance's GUI and commands from an allowlist, as a capability a window or command must hold, not as checks scattered through each feature, which is NVDA's denylist and what each of those advisories slipped past. Windows' own error dialogs count too: NVDA had to turn off wx's logging dialogs because one led to a file dialog.
- **S5. Measure the UIAccess boundary first.** On the target Windows build, record what an ordinary process can do to a UIAccess Verbatim: open its process, duplicate its token, send it window messages, and whether its injected keys reach Verbatim's hook with `LLKHF_LOWER_IL_INJECTED` set. The record goes in `experiments/`, and these rules are adjusted to the result.
- **S6. Remote support (M12) on secure screens follows the same rules.** No remote input reaches a secure screen unless the local user allowed it there, and no local program can stop a permitted remote session from reaching it (NVDA's GHSA-vr27-g5ph-xvq2 let a planted file do that).
- **S7. Whether Windows is locked is asked of Windows when it is decided, and an unknown answer counts as locked.** Reason: NVDA judged the lock from the foreground process, which another window could take over (GHSA-rmq3-vvhq-gp32), assumed an unlocked session when it started on the lock screen (GHSA-72mj-mqhj-qh4w), and today treats an object as safe when its check raises an error. Read the session state from Windows' session notifications and query it at startup.
- **S8. On the lock screen in the user's own session, only an allowlist of commands runs, and navigation stays inside the lock screen.** Reason: NVDA's first lock-screen advisory was object navigation walking out of the lock screen onto the desktop (GHSA-x2c3-c6j2-87w4). Its answer, an allowlist of safe commands and objects whose neighbours outside the lock screen read as absent, is the right shape; Verbatim's reducer is the one place every command passes, so the allowlist lives there.
- **S9. Nothing of the user's session follows them onto a secure screen.** The speech history, the last braille text and any viewer clear on the switch and at exit. Reason: NVDA leaked its speech viewer's text and the braille display's contents this way (GHSA-grvr-j2h8-3qm4, GHSA-8f8q-2jc3-6rf4).
- **S10. Verbatim never runs elevated on the user's desktop.** Started from an elevated prompt, it restarts itself with the user's ordinary token or refuses to start. Reason: NVDA started elevated after an install from an elevated prompt, which made its console an administrator's (GHSA-cq78-fjp7-3rp5). Today Verbatim's manifest asks to run as the invoker, which inherits an elevated invoker's token.
- **S11. Every executable Windows starts for Verbatim on the secure desktop is administrator-owned and signed.** Reason: the accessibility tools Windows starts at sign-in are a known route to a SYSTEM shell, by replacing the binary or redirecting it through the "Image File Execution Options" key (MITRE ATT&CK T1546.008). The registration Verbatim writes names an absolute path inside the installation.

### 4.7 Extensions and native code

- **E1. Wasm extensions keep the capability model of [extensions](extensions.md), section 9.** Capabilities are derived from imports, no WASI is offered, epoch deadlines bound compute, store limits bound memory, and the outpost's job caps the rest. This is a strong position: NVDA's add-ons are unrestricted Python in NVDA's own process.
- **E2. Native code runs only in a contained synthesizer host.** The AppContainer in its less privileged form is planned for M6 and not built yet; until then the host runs with the user's full token. Native adapters load only from an administrator-owned folder in installed copies.
- **E3. An extension cannot change speech in Verbatim's own windows, elevated windows or secure screens.** Reason: rewriting speech is a deception channel that is unique to screen readers. The extension design already refuses claims on Verbatim's own executables and limits secure screens to an administrator-made copy. Add elevated applications, unless the user allows a module there by name.
- **E4. Nothing installs or loads an extension without the user's confirmation in Verbatim's own dialog.** Today the design watches the developer folder always, so any program can drop a module there and it loads with no prompt, bypassing the install dialog. Proposed: watch it only in developer mode (question 6).
- **E5. A Wasm runtime escape is contained by the outpost process.** Outposts cannot run in an AppContainer, because Windows denies such a process UIA and MSAA client access to other processes. They are to get restricted tokens and process mitigations, measured for what UIA still allows. Wasmtime's compiler needs dynamic code, so the mitigation that forbids it cannot apply to an outpost hosting modules unless precompiled artifacts are loaded as images; record that trade-off when the mitigations are chosen.

### 4.8 Hardening: memory safety, crash safety and the supply chain

- **H1. Unsafe code stays in the Windows crates, behind small safe wrappers.** Today the platform-neutral crates forbid unsafe code at the crate level, except where a crate's guide says otherwise. Unsafe is concentrated in `verbatim-uia` (about 195 uses), `verbatim-outpost` (123), `verbatim-process` (96), `verbatim-ia2` (74) and `verbatim-app` (70). The test tools `verbatim-agent` and `mockapp` hold most of the rest and never ship.
- **H2. C and C++ that parse untrusted input run outside Core.** eSpeak NG parses text from applications and runs in the synthesizer host, which is the right place, and it gains the sandbox in M6. wxWidgets in Core displays Verbatim's own data, and extension names and log lines, which are short and Verbatim-controlled.
- **H3. Core code does not panic on application data.** Today `verbatim-core` and `verbatim-text` have no `unwrap` or `expect` outside tests. The remaining slice and index sites in the reducer and terminal code are candidates for fuzzing rather than audit by eye (rule H4).
- **H4. Every parser and every algorithm that takes application data is fuzzed.** Reason: tests written by hand cover the cases their author thought of. Proposed targets, all platform-neutral and so able to run on the Linux job: the frame and raw-section decoders, the control-plane message reader, the configuration, theme and package readers, terminal alignment and segmentation, and the reducer fed generated inputs. Add a `mockapp` mode serving hostile trees (self-parents, cycles, a million children, multi-megabyte names, providers that hang) as end-to-end checks of rules T1 to T6.
- **H5. Dependencies are audited.** Add `cargo deny` (advisories, licences, sources) to `cargo xtask ci`. The licence check also serves the platform-neutral tier's licensing rule.
- **H6. The injection helper (M7) is the most sensitive code Verbatim will ship.** It runs inside browsers, so a defect in it crashes or compromises the user's browser. It is to be minimal, in Rust, loaded only from the administrator-owned installation, and its channel to the outpost treats the browser's side as hostile.
- **H7. No text from an application is ever rendered as markup or code.** Message dialogs and viewers show application text as plain text. Reason: NVDA's HTML message dialog turned web text into code run on two key presses (GHSA-xg6w-23rw-39r8).
- **H8. Updates are verified by signature, not only by hash.** The installer and updater (M8) check the download's Authenticode signature against Verbatim's own certificate before running it, and the installer pins its DLL search as rule S2 says. Reason: NVDA's updater checks a SHA-1 hash supplied by the same server, and its installer loaded DLLs planted beside it (GHSA-qf5h-qw92-rx2f).

## 5. How well the architecture serves the goal

The architecture is strongest exactly where screen readers have historically been weakest, and weakest in product surfaces that have not been designed yet.

Strengths:

- **Isolation from hostile applications.** One outpost per application (D9), a listener that makes no cross-process call in its callbacks (D13), deadlines on every call, the recovery ladder, and job objects with memory caps. A hostile or broken application costs reading that application only. This is the architectural answer to the first threat class, and NVDA has no equivalent.
- **A pure reducer with no COM.** Core sees only normalized data. That makes rule T5 enforceable at one seam, and makes Core's logic fuzzable on Linux.
- **Private channels.** Core's channels to its children are unnamed pipes passed by handle, so there is nothing to discover or to protect with an access list.
- **Extensions as Wasm with derived capabilities (D6).** The extension surface can be listed from the binary, compared with NVDA's unrestricted Python add-ons.
- **Native engines out of process (D18),** with a sandbox designed per engine.
- **No injection until M7, and then through one helper (D2).**
- **Rust with unsafe confined to the Windows crates.**
- **Configuration that cannot name code.**

Weaknesses, all fixable without changing the architecture:

- Control surfaces are on by default and broad, and the test agent is open to the network.
- Diagnostics keep content and are stored beside the executable, with no masking, owner or end.
- The installed-copy file layout assumes a writable executable folder, which conflicts with Program Files and UIAccess.
- There is no repeat detection in walks, no cap on property strings, a quadratic step in Core's terminal code, and no memory cap or restart for Core.
- There is no fuzzing and no dependency audit.

## 6. Findings in today's code

Ranked by severity. Severity is judged for today's development use; the items marked as rising with UIAccess become blockers for M8.1.

- **F1, critical on any networked development machine or VM.** `verbatim-agent` listens on every interface with no authentication and can launch processes and write files. Rule C4.
- **F2, high, rising with UIAccess.** The control plane is always on and offers key injection, the speech stream and dumps. Rule C1.
- **F3, high for privacy.** Crash dumps hold typed characters, including those typed into password fields, and copied text, beside the executable, and are kept forever. Rules P1, P2 and P4.
- **F4, medium.** Settings, logs and dumps live beside the executable. There is a fallback to `%TEMP%\verbatim-defaults` and another to the current directory. Rules C3 and S1.
- **F5, medium.** Any Low-integrity process can end Verbatim with `WM_QUIT`. Rule A3.
- **F6, medium.** Gestures accept injected keys from any integrity level. Rule I1.
- **F7, medium.** Terminal alignment in Core is quadratic in row width, Core has no memory cap, and the input reaches it from remote programs' output. Rules T4 and T6.
- **F8, medium.** No DLL search restriction or process mitigations in any executable. Rule S2.
- **F9, medium.** No Core restart after a crash. Rule A2.
- **F10, medium.** No cap on synthesizer restarts, and no fallback after startup. Rule A4.
- **F11, low.** No repeat detection in walks; a self-parent is spoken up to 64 times. The classic UIA child listing is capped only after fetching. Rules T1 and T2.
- **F12, low.** Name, value and description strings are uncapped up to 32 MiB, and the 64 KiB text cut is silent. Rule T3.
- **F13, low.** The control pipe's and configuration's readers have no size limit. Rule C3.
- **F14, medium, rising with UIAccess.** Started from an elevated prompt, Verbatim runs elevated. Rule S10.
- **F15, process.** No fuzzing, no `cargo deny`. Rules H4 and H5.

## 7. Where the work goes

- Now, as a small build of its own: F1 (the agent), F5 (the message filter), and the configuration fallbacks of F4.
- M5: rule E4 (the developer folder), the user and developer endpoints of rule C1 (the install request arrives with M5), and rules P1 and P2, since M5 brings the log ring forward.
- M6: rules A4 and E2, and the AppContainer as designed.
- M6.3, the speech and audio pipeline: rule T3's flag reaching speech.
- M6.4, terminal residuals: rule T4.
- M7: rules H6 and H7, and rules T1 and T2 before the IA2 tree walks multiply.
- M8, with the installer skeleton: rules S1, S2, S10, P4, A2 and H8, and the per-user state location.
- M8.1: rules S3 to S9 and S11, and I1 to I3, starting with the measurement of rule S5.
- M9: rule P3 as file logging arrives.
- M12: rule S6 and the remote transport's pairing.
- Ongoing, from now: rules H4 and H5 in `cargo xtask ci`, and rule T6's background flood cap with the next focus build.

## 8. Comparison with NVDA and other screen readers

NVDA's mechanisms below were read from its source at the pinned commit. Its advisories come from its GitHub advisory list, which held 24 entries on 2026-10-31. The other readers' history comes from NVD, vendors' documentation and published research. The detailed survey with its sources is in [the security comparison](../../experiments/security-model-survey/readme.md).

### 8.1 NVDA's record

About half of NVDA's advisories are escapes from secure screens or the lock screen, several rated critical: the Python console, the log viewer, context help, a clipboard error dialog and a formatting dialog each led to code running as SYSTEM, and object navigation walked out of the lock screen. The rest are information left behind across that boundary (debug logging, the speech viewer, braille contents), code execution (DLL planting in the installer, web text run through an HTML dialog, an add-on able to patch NVDA's elevation code, which NV Access treats as a consequence of add-ons being unrestricted), and Remote Access flaws, the worst a missed certificate check on reconnecting.

The lesson Verbatim takes is that the secure-desktop instance, not the everyday one, is where a screen reader is attacked, and that a denylist of features there fails one feature at a time.

### 8.2 Mechanism by mechanism

- **Isolation from hostile applications.** NVDA runs its logic on one main thread, and its watchdog cancels COM calls and `SendMessage` after a stall. Verbatim isolates each application in its own process (D9). Verbatim is ahead. Both cap ancestor walks (NVDA at 100 hops, then it jumps to the desktop; Verbatim at 64), and neither detects a repeat (rule T1).
- **Control surface.** NVDA's controller is a local RPC endpoint whose access list grants full access to Everyone and to every AppContainer, with no authentication, and carries both the public speech functions and internal ones, including an add-on install request and a Remote Access link handler. Its injected helper reports through the same endpoint. Verbatim's pipe admits only the owning user and refuses remote clients, so it is ahead on reach, but it offers key injection, which NVDA's controller does not (rule C1).
- **Logs and dumps.** NVDA's log at its IO level records every utterance and every key, including keys typed in password fields. It lives in the temporary folder, and only its startup configuration dump is redacted. In secure mode its logging is off. Verbatim logs no content today, which is ahead, but its crash dumps keep typed characters and the clipboard in readable form (rules P1 and P2). NVDA's crash handler writes a minidump of its memory, which holds whatever it was reading, so neither keeps content out of crash data.
- **Protected fields.** Both echo a star for each typed character and suppress typed-word echo.
- **Injected input.** Both treat injected keys as gestures. NVDA lets the user turn that off, and neither looks at the injector's integrity level (rule I1).
- **Extensions.** NVDA's add-ons are unrestricted Python in its process. Its store pins each release by SHA-256 and scans it, but nobody reviews the code and nothing is signed. Verbatim's Wasm modules with derived capabilities are ahead by design (rule E1). NVDA's copy of add-ons to the secure screens, chosen by the user and made by an elevated step, is the model Verbatim's design already follows.
- **Configuration.** NVDA keeps it in the user's profile for installed copies, beside the executable for portable ones, and in an administrator-owned copy for secure screens; it validates the file against a specification and resets a corrupt one to defaults. Verbatim matches this only once rules C3 and S1 are done.
- **Recovery from its own crash.** NVDA's crash handler restarts NVDA, with a breaker that stops after too many crashes in a short window. Verbatim does not restart (rule A2), and should copy the breaker.
- **Synthesizers.** NVDA runs most synthesizers in its own process. A failure to load falls back from OneCore to eSpeak NG to silence, but a crash at runtime takes NVDA with it, and only 32-bit SAPI voices run in a separate host. Verbatim's host per synthesizer (D18) is ahead. Neither falls back after a runtime crash (rule A4).
- **Secure mode.** NVDA's is mature, built from twenty advisories. Verbatim has none yet. It has the chance to build one from an allowlist from the start (rules S3 to S11).
- **Updates.** NVDA checks a SHA-1 hash from its own update server and runs the launcher; no signature check was found. Verbatim has no updater yet (rule H8).
- **Injection.** NVDA injects its helper DLL into every process that raises a focus or foreground event, through global in-context hooks. Verbatim injects nothing until M7, and then only through one helper (D2).

### 8.3 Other screen readers

- **JAWS.** Its scripts are compiled code loaded per application. Its one public vulnerability found is a service, in versions 13 and earlier, that any user could reconfigure to run code as SYSTEM. No other advisory or CVE was found, which may reflect disclosure practice rather than an absence of flaws.
- **Narrator.** It was the route of a BitLocker lock-screen bypass (CVE-2020-1398), and NVD lists several privilege-escalation entries for Narrator's braille support and for the accessibility broker, `ATBroker.exe`, in 2026, chiefly untrusted search paths. Even Windows' own screen reader is attacked at the sign-in screen and through DLL search order.
- **Orca and AT-SPI.** The Linux accessibility bus has no access control, and any client can see every other's events. GNOME's recent work moves keyboard capture into the compositor and sends keys only to an allowlisted screen reader, so that no process can act as a keylogger. The lesson for Verbatim is that a screen reader's input path is a keylogger by design, and must not be readable by others (rule C1's speech stream, rule P1).
- **VoiceOver.** Apple's fixes include VoiceOver reading secure input fields, saved passwords and the passcode aloud, and a login window bypass. macOS asks the user to grant accessibility access per application. The lesson is that protected text must never be spoken by any path, including paths added later (rule P2).
- **Android accessibility services.** Banking malware uses accessibility services to read the screen and inject input, which is why Android 13 restricts them for sideloaded apps, a restriction droppers have bypassed. Any API that grants a screen reader's reach is a malware target: Verbatim's extension imports must not hand that reach to an extension the user did not knowingly install (rule E4).
- **Research.** Jang and others (CCS 2014, "A11y Attacks") bypassed UAC and the sandboxes of four systems through accessibility. On Windows, speech synthesized by a process drove Speech Recognition, which runs at High integrity, and UI Automation clicked a password field's reveal button. Their recommendation, that an assistive technology check that its input comes from the user, is rule I1.

### 8.4 Where Verbatim can do better

- Build secure mode from an allowlist before the secure instance first runs, rather than after advisories.
- Decide lock state from Windows, failing closed.
- Give the control surface the least reach, and none on secure screens.
- Keep content out of every file by default, including crash dumps.
- Refuse lower-integrity injected input, which no surveyed screen reader does.
- Fall back to another synthesizer after runtime crashes, which neither NVDA nor Verbatim does today.
- Verify updates by signature.

## 9. Questions for Dickson

1. **Control plane.** Split it into an always-on user endpoint (the install request and status only) and a developer endpoint that exists only in developer mode? And should signed builds with UIAccess carry the developer endpoint at all, given that its key injection would reach elevated windows? Recommended: split, and keep the developer endpoint in signed builds behind developer mode, refusing `SendKeys` while a window of higher integrity than the client is in front, so end-to-end tests can still run against a release build.
2. **Injected gestures.** Ignore Verbatim gestures in keys injected from a lower integrity level, and keep accepting same-level injected keys as NVDA does? Recommended: yes.
3. **Crash dumps.** Content-free automatic dumps, with content only in a dump the user asks for? Recommended: yes, with masking of protected-field input at record time regardless.
4. **Installed state.** Settings, logs and dumps under the user's profile for installed copies, and beside the executable for portable copies, as NVDA does? Recommended: yes.
5. **Synthesizer fallback.** After three crashes within a minute, switch to eSpeak NG for the session and say so? Recommended: yes.
6. **Developer folder.** Watch the extension developer folder only in developer mode? Recommended: yes.
