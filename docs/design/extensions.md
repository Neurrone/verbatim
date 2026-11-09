# Extensions: the design

The design of Verbatim's extension system, the equivalent of NVDA's app modules, add-ons and synth drivers, settled on 2026-10-22 and restated with Dickson on 2026-11-09. It is implemented by M5 ([work plan](m5-extensions-work.md)) and, for native speech engines, M6 ([work plan](m6-native-synth-work.md)). D6 in the architecture is the decision of record; this document is its detail. Where a sentence carries a name and a date, it is a decision Dickson took and is not reopened without new evidence.

The surface as extension authors see it, every manifest key, matcher form, rule action, hook, event kind, import and install dialog line, is the Extensions guide's reference (`books/extensions/src/reference/readme.md`), and the WIT files in `verbatim-ext-api` once they exist, from which that reference is generated. This document does not restate it: it records how the surface is implemented and why.

## 1. Terminology

- An **extension** is the installable unit: one `.vext` file, a zip its author makes by hand holding an extension manifest, locale files and one or more module folders. NVDA's add-on.
- A **module** is one contribution inside an extension, of a **kind**: an **app module**, bound to one or more applications and active while such an application has an outpost; a **global module**, bound to none, one instance for the session, reading only the applications its manifest's `reads` names; or a **synthesizer**. Speech-text stages, recognisers and braille drivers are later kinds.
- A **built-in** is a module folder that ships with Verbatim under `extensions/builtin/` (planned). The built-ins together are one internal extension, loaded and run as an installed one is, but never listed, installed or disabled by the user.
- A **rule** is a matcher and an action Verbatim applies itself, without calling the module; a **hook** is a function a module exports and Verbatim calls, with a deadline.
- A **capability** is a WIT interface a component imports, derived from its imports and never declared.
- The **host** is the process running one extension's modules, `--extension <id>`; a **lane** is one thread in it, one per outpost it serves and one per global module.
- The **loader** is a thread of Core's that reads, validates and compiles extensions and reports each result to the reducer.

## 2. Extensions, modules and built-ins

### 2.1 One folder layout

A module is always a folder containing `module.toml` and, unless the module is rules only, a component named in it. A built-in is that folder under `extensions/builtin/` (planned) with no extension manifest of its own, its strings in Verbatim's own Fluent resources. An installed extension is a `.vext` zip with `manifest.toml` at its root, a `locale/` folder and its module folders; there is no packaging tool, and the guide documents the zip (Dickson, 2026-11-09). Install extracts it to `extensions/<extension id>/` under the configuration root, the user's profile for an installed copy and beside the executable for a portable one, first into a hidden staging folder beside its destination, renamed into place only once every file is written and checked ([security model](security-model.md), X13). The loader has one code path for a module folder wherever it finds it.

### 2.2 Loading, the developer folder and hot reload

The loader thread does all file and processor work: it reads folders and `.vext` files, parses and validates manifests and rules with `verbatim-ext-api`, reads each component's imports to derive its capabilities, and compiles the merged rule tables with `verbatim-reader-rules`, then asks the extension's host to compile and pre-link its components. It reports a finished result to the reducer, validated and ready at a version, or failed with its reason, never a step in between. The reducer owns which version of each extension is active, one state value per extension (`Active` at a version with its tables, `Failed` with a reason, or `Disabled` by the user or the breaker), and publishes each new version: rule tables and hook registrations to the listener and the outposts, the gesture map to the keyboard hook thread, a swap to the host. Each outpost takes a new version at a boundary in its intake, so every event is judged under one version, and Core recognises a judgement made under a version it has moved past, by the ownership model's version rule. The host drops an old instance once no outpost still calls it. A reload that fails at any step leaves the previous version everywhere.

The developer folder holds unpacked module folders and is watched, with the built-in folder, only in developer mode. A developer-folder module that claims an application an installed extension claims replaces that extension's module for that application only, while it is there, as NVDA's scratchpad does (`addonHandler/packaging.py`, `addDirsToPythonPackagePath`), so an author can work on the next version of an installed extension (Dickson, 2026-11-09); two developer-folder modules claiming one application fail the later. Speech says "Reloaded <module>", or "Reload of <module> failed" with the first line of the reason.

A built-in failing to load is a defect or a damaged install, logged and spoken once as "Built-in module <name> failed to load". Installed extensions' failures are one sentence, "<n> extensions failed to load", with each marked failed in the Extensions dialog.

### 2.3 Secure mode

Secure mode (M8.7) loads only a machine-wide copy of the extensions the user allowlisted for secure screens, an allowlist of plain data, made by an elevated step into a folder the installer made readable for the hosts' containers, as NVDA copies selected add-ons into its system configuration (`config/__init__.py`, `_setSystemConfig`); installation, the developer folder and the console are disabled there. Allowlisting is the user's consent, and the allowlist dialog shows the extension's warnings as the install dialog does (Dickson, 2026-11-06). An extension on a secure screen writes only a scratch folder, deleted when Verbatim exits.

## 3. Binding a module to an application

The supervisor resolves each outpost's application identity at launch from a fixed core table: the executable name; for a console window, the console client's executable; for `wwahost.exe`, the hosted application's name from its application user model id; for `msedgewebview2.exe`, the parent process's executable. The listener learns a process's identity the same way, with one cached image-name query per process id, so window and event rules apply before any outpost exists. A module's `applications` list is NVDA's alias table in the module's own manifest.

At most one installed extension claims an application, above the built-in, and no two modules of one extension claim the same one; a second is refused at install, so installed extensions need no order among themselves (Dickson, 2026-11-09). A host runs one instance per outpost of a claimed application, on that outpost's lane, as NVDA keeps one `AppModule` per process; the instance's memory is its state, lost on reload or fault. A global module has one instance on a lane of its own, and its reads reach only the applications in `reads`; when it reads an application with no outpost, the supervisor starts one.

## 4. The surface, as implemented

The surface is the guide's reference. These are the implementation rules behind it.

- **One matcher grammar,** one type, parser and evaluator in `verbatim-ext-api`, serving rules, hook registrations and the find reads; legality per use is a check at load. Window and event rules need a property named directly at the top level, so they compile to hash tables and stay one probe per event.
- **Rules are placed by the loader from the properties they name:** a focus veto naming only what the listener's event shape (`ListenerEvent`) caches runs in the listener before its read, and otherwise in the outpost on the read node; an event drop runs in the first reader that has its properties, and otherwise in Core; node rules and redirects run in the outpost's worker as it emits a snapshot; window `backend` and `normalize-class` where a reader first reads the window; `silent` and `accept` in Core's reducer.
- **Hooks are called from the outpost's worker,** which waits for the host's answer within the event's deadline and serves the host's reads meanwhile: `on-focus` after the listener's read and the node rules, `on-event` after the rules, `on-node` once per emitted batch after the node rules. `status-bar` and an app module's `on-command` are resolved by the reducer and run through the outpost in key order; a global module's `on-command` and every `on-list-dialog` go from the reducer straight to the host.
- **Reads** are bindings over the outpost's own readers, so a module sees what its outpost sees; every navigation stops at the application's boundary, checked by the node's process in one place, and the outpost judges each request by the pipe it arrived on, so the extension is the unit whose reach is enforced.
- **Effects** go to the reducer: speech judged by attention, a list dialog recorded in the owning lane's record and shown by the GUI thread, a click answered by asking the node's outpost where the element is now and then moving the pointer and clicking from a shell thread, as the tray list does today.
- **Commands:** a binding names a number of presses; the hook thread stamps each raised gesture with when its multi-press window closes, every press fires at once, and a list dialog a command opens is held in its lane's record (`Held` until that time, or `Open`) when the same keys are bound to a higher count, so a double press never shows the first dialog (Dickson, 2026-11-09). `trigger.declared` names the binding a press resolved to, which a user's change of gesture keeps.
- **Node handles** are component-model resources owned by the instance; the host reports the nodes it holds, so the outpost keeps their elements alive.
- **Gesture conflicts** are listed in the install dialog, not refused (Dickson, 2026-11-09).
- Accepted: a module may remove the protected state, where an application sets it wrongly; the provenance names the module that did.

## 5. Where modules run

- **Every extension's host,** the built-ins' included, runs that extension's modules on its lanes and makes no call into any application. It runs in an AppContainer, with a memory cap, and a processor rate cap unless it holds a synthesizer ([security model](security-model.md), X4, A7). The built-in host is started with Verbatim and always runs, since the taskbar and global modules need it (Dickson, 2026-11-09).
- **The outposts** apply node rules, call hooks from their worker and answer hosts' reads; they load no runtime.
- **The listener** applies window rules, focus vetoes and event drops as data, and runs no module code.
- **Core** owns the extension lifecycle, publishes the tables and the gesture map, judges and performs effects, and shows dialogs; it runs no module code and links no Wasm runtime.
- **The synthesizer hosts** run eSpeak NG and OneCore as D18 has them; whether they become built-in extensions behind the synthesizer C ABI is open (section 13).

Core's reducer stays pure: a module's speech is an ordinary effect, and a module command and a dialog's answer are recorded inputs, so the flight recorder's replay needs no module.

## 6. Ordering, provenance and conflicts

For one application there are at most three layers: a developer-folder module or the one installed extension, then the built-in, then core behaviour. Within one event, every layer's rules apply first, lowest layer first, so the highest layer's action wins a contested property; then hooks run from the highest layer down, `pass` continuing the chain, a drop never undone (Dickson, 2026-10-28). For commands, the highest layer that binds a gesture gets it; the user's own gesture map sits above every layer once it exists. The outpost records for each overridden field which layer and rule set it, and the focus report shows it.

## 7. Performance

A rules-only module costs what the constant tables cost today: rules are merged once at load into the tables the listener and the outposts consult, keyed by identity. A hook costs a round trip between processes and two wake-ups on the event's path, and each read two more, accepted for built-ins as for installed extensions (Dickson, 2026-11-09), measured in step 4 against D15. Per event, the time inside module code, split into guest and host-call time, is a stage of the latency timeline, and the call ledger carries two rows ratcheted against `mockapp`: a rules-only module leaves every count unchanged, and a module with one descendant read adds exactly one UIA call.

Wasmtime's epoch interruption bounds a hook's own execution; since the epoch is the engine's and one host runs many lanes, each store's deadline callback checks its own hook's time before it traps. The compute bound is loose until step 8 sets it from the built-ins' measured guest time; the total bound is the event's remaining handling deadline, ten seconds, enforced by refusing a read once spent. A trap counts as `pass` or no change, the instance is re-created, and the breaker (A9) counts it.

## 8. Capabilities

Capabilities are derived from the component's imports for built-ins and installed extensions alike, and the install and Extensions dialogs are built from them, as the guide's capabilities page lays out and words them (Dickson, 2026-11-09). An install request reaches the running Verbatim through the user endpoint from a small helper executable registered for `.vext` by the installer (M8.6), which shows a message box that Verbatim must be running when nothing answers, as NVDA's `nvda_slave.pyw` does; the Extensions dialog's Install from file button needs no association, and is how a portable copy installs.

## 9. The runtime and the guest workflow

The runtime is wasmtime with the component model (Dickson, 2026-10-22), chosen by a requirements comparison: Windows x64 and ARM64, the component model and WIT, interruption of running code, memory limits, precompiled artifacts; wasmer dropped its Windows native compilers, wasmi and WAMR have no component model, WasmEdge is x64 only. Cranelift compiles on both architectures; Pulley is switched on for ARM64 only if `ci-arm64` ratification or a Cranelift fault requires it; Winch is excluded because it cannot be epoch-interrupted. `verbatim-ext` is the only crate that names the runtime.

The WIT files in `verbatim-ext-api` are the single source (Dickson, 2026-11-09): `wasmtime::component::bindgen!` generates the host's traits, `wit-bindgen` the guests' bindings, and an xtask step parsing them with `wit-parser` generates the guide's API reference pages, which `cargo xtask ci` checks are current; the manifest reference and a JSON Schema are generated from the manifests' `serde` types with `schemars`, with the same check. The conversions between Verbatim's internal types and the WIT types are exhaustive matches, so a new internal variant fails to compile until someone decides how modules see it. Guests build for `wasm32-unknown-unknown` with no WASI adapter, in a workspace separate from the host's; the guide's examples are crates there, included into the book, so its code is compiled and tested.

## 10. Versioning

A WIT package carries a semantic version, and every interface a component imports is named with it in the binary; the linker resolves an import by exact name first and otherwise by track, several tracks living side by side for a deprecation period, and `wasm-tools component semver-check` proves each new version compatible. The manifest's `api` names the version an extension was built against and must agree with the binary's imports, which are the authority. There is no `verbatim-minimum` or `verbatim-tested`: NVDA needs its version fields because its API breaks each year, and the API's tracks already say what an extension needs (Dickson, 2026-11-09).

## 11. The console

In M5, `verbatim-inspect call` invokes any function of the world by name with WAVE-encoded arguments through a host's dynamic call path, relayed by Core's developer endpoint. The interpreter console and scripted modules are M10's; with no runtime in Core, the console runs in a host of its own, and its design, settled in M10, starts from that.

## 12. Synthesizers and native code

Native code comes in two forms (Dickson, 2026-11-09): a library a Wasm module of any kind calls through a narrow import that passes bytes to a named function in a DLL its extension ships, in the extension's host, which covers NVDA add-ons calling a vendor SDK; and a whole native module for a kind with a small interface, implementing that kind's C ABI, first the synthesizer's, `verbatim_synth_v1`, the `SynthDriver` trait as C exports (version, create, destroy, describe, set a setting, speak a sequence through PCM, mark and is-cancelled callbacks, cancel). A native synthesizer's manifest names one adapter library per architecture. An engine's files that may not be distributed, such as Eloquence's, are added by the user to the extension's folder before zipping it; there is no `user-files` list (Dickson, 2026-11-09). The x86 host for 32-bit engines carries no Wasm runtime, so an extension whose native code is built only for x86 holds no other module. Native code runs in its extension's host, contained as section 5 says, and the install dialog warns that it "Contains native code that runs outside Verbatim's extension sandbox", the extension sandbox being the Wasm boundary.

Open: whether eSpeak NG and OneCore become built-in extensions, each a native synthesizer module in a host of its own behind `verbatim_synth_v1`, in place of `verbatim-synth-host`'s engine mode, so that every host process is one extension's host. Their sandbox levels differ and an engine's crash must not stop the other, so they stay separate processes either way.

## 13. Porting NVDA's app modules

The porting track runs from M5 onward. In M5, every Windows inbox application: Explorer, Windows Terminal (a focus veto dropping every `TermControl` focus whose runtime id starts 14, with no hook; Dickson, 2026-11-09), Notepad, Calculator, Task Manager (proven by `mockapp` until UIAccess), Settings, Start and search, Quick Settings and Open With, and the tray list, taskbar list and time as global modules. Sleep mode becomes a core per-application list seeded with the five executables NVDA's sleep-only modules name. A module is done when its NVDA behaviours are captured on the VM with the pinned build, one end-to-end scenario per behaviour asserts the captured utterance, its own tests pass, and the parity ledger lists each behaviour with the scenario that pins it. After M5 the order of the track is appendix A's tiers.

## Appendix A: NVDA's app modules, one line each

Pinned source `nvda/source/appModules`, 78 modules. For each: target and status, size in lines, mechanisms, and what the surface needs. Alias executables are given where NVDA's table maps them. Tier A needs only the surface above; tier C waits for browse mode and IA2 (M7); tier D waits for an object-model bridge; tier E is a binding rule.

- 1password (21). 1Password 7 UIA event flood; version 8 is Electron. Drops Name, ItemStatus and IsEnabled property changes. Skip.
- aim (8). AOL Instant Messenger, discontinued. Skip.
- audacity (19). Current. Init hook strips ampersands from button names by class and role. Tier A, a rule.
- azardi20 (44; also azardi-2.0). Abandoned EPUB reader. Skip.
- bookshelf (46). The old Internet Explorer based reader. Skip.
- calc (86; also win32calc). Legacy Win32 calculator. Skip.
- calculator (190; also calculatorapp). Windows 10 and 11 Calculator, current. Init hook names History and Memory items from children; `DisplayUpdated` notification with its own UIA query; name-change filter by automation id; result keys that pass through. Tier A.
- cicero (7). Legacy self-voicing reader. Sleep only; the core sleep list.
- code (127; also VS Code Insiders, Azure Data Studio). VS Code, current. Blocks browse mode on the Chromium document, replaces text info, status bar by IA2 id search. Tier C.
- csrss (21). Marks the system console process alive. Not applicable.
- devenv (250; also ssms). Visual Studio, current. DTE object model, custom text info, LiteTreeView32 overlays. Tier D.
- digitaleditions (53; also the preview). Adobe Digital Editions. Page-turn document, synthetic Page Up and Down. Tier C.
- dllhost (22). Early Windows 10 property sheets. Skip.
- doctts (5). Unidentified self-voicing app. Sleep only; the core sleep list.
- dosvox (5). DOSVOX. Sleep only; the core sleep list.
- eclipse (208; also DBeaver, Spring Tool Suite, Zend). Current. Focus veto on unfocused tree items, an editable-text-with-suggestions overlay with three scripts. After M16.
- egui (16). Probably an older ESET GUI. Skip.
- esysuite (17; also esybraille). Eurobraille suite. Sleep only; the core sleep list.
- excel (144). Current. Good-UIA by version, overlays with a reparenting override, a focus redirect that polls, display-model edit box. Tier D.
- explorer (551). Current. Ten overlays, init hook naming, focus drops for staging windows and pending menus, tab-switch announcement, the snap notification, good-UIA roots, status bar, product info. Tier A, first.
- fastlogentry (25). Niche ham-radio logger; display model. Skip.
- foobar2000 (210). Current. Caches the status bar on focus; three scripts speak elapsed, remaining and total time. Tier A, on the track.
- hxmail (57; also commsapps, hxoutlook). Mail and Calendar, ended 2024. Skip.
- instantbird (25). Discontinued. Skip.
- itunes (123). Legacy. Skip.
- javaw (53). Host resolution for Java applications by parent process and command line. Tier E, after M16.
- kindle (464). Kindle for PC 1.x book view, IA2. Custom browse-mode interceptor. Tier C.
- lockapp (106). Windows lock screen, current. Overlays, input capture, review position save and restore. After M8.7.
- lockapphost (15). Legacy. Skip.
- logonui (129). Windows logon, current. Overlays, focus re-queue, password field name. After M8.7.
- loudtalks (42). Renamed Zello. Skip.
- lync (99). Skype for Business, discontinued. Skip.
- microsoftedge (20), microsoftedgecp (32). EdgeHTML, discontinued. Skip.
- mintty (7). Current; subclasses putty with its window class. Display model; M17.
- miranda32 (282; also miranda64). Custom window messages with remote memory. Skip.
- mirc (41). Current. Display-model live text. M17.
- mmc (80). Management Console, current. `AfxWnd42u` overlays, synthetic focus from selection, bad-UIA list view. Tier A, later.
- mplayerc (14; also mpc-hc). Fork maintained. Init hook sets a status bar role by class and control id. Tier A, later.
- msedgewebview2 (25). WebView2 host resolution by parent process. Tier E, in M5's identity table.
- msimn (165; also winmail). Outlook Express, discontinued. Skip.
- msnmsgr (104). Discontinued. Skip.
- nlnotes (60). HCL Notes. In-process virtual buffer. Skip.
- notepad (84). Current. Tab-switch announcement, Windows 11 status bar query, `LineNumberBox` role. Tier A.
- notepadPlusPlus (73; also notepad++). Current. Scintilla text info by messages, IME composition text info. Core Scintilla provider first, then tier A.
- nvda (257). NVDA's own GUI. Not applicable.
- obu (13). OpenBook. Sleep only; the core sleep list.
- openwith (54). Current. Good-UIA for `Shell_Flyout` and `Open With`, two overlays. Tier A.
- outlook (791). Classic Outlook, current. Good and bad UIA by version, `requestEvents`, COM object model, in-process MAPI reads, custom browse mode for mail. Tier D.
- poedit (297). Current. Four scripts with repeat counts and a browseable message, tones, lookups by control id. Tier A, on the track.
- powerpnt (1743). Current. COM object model with event sinks, a synthetic object tree, slide show interceptors. Tier D.
- putty (34). Current. Display-model terminal. M17.
- searchui (98; also searchapp, searchhost). Start and search, current. Bad-UIA for the Chromium render widget, overlays with ancestry overrides, suggestion count name. Tier A, partly later.
- securecrt (31). Current. Display-model terminal. M17.
- shellexperiencehost (83; also shellhost). Quick Settings, current. Good-UIA `ControlCenterWindow`, toggle values from item status, calendar day headers. Tier A.
- skype (12). Discontinued. Skip.
- soffice (653). LibreOffice, current. IA2 text and table interfaces, compound documents. Tier D, after M7.
- spotify (42). Current. Focus veto on negative child ids, focus redirect to the active descendant. Tier C.
- symphony (21). Discontinued. Skip.
- systemsettings (30). Settings, current. Removes the progress bar behaviour from volume meters. Tier A.
- taskmgr (47). Current. Presentation type for `TmViewRow` children, bad-UIA list view. Tier A.
- teams (33). Classic Teams, retired. Skip; new Teams is tier C.
- teamtalk4classic (23). Legacy. Skip.
- thunderbird (58). Current. Menu item names from IA2, status bar on a busy document, name change forwarded as state change. Tier C.
- totalcmd (49; also totalcmd64). Current. Panel announcement, `reportFocus` override. Later.
- ttermpro (27). Tera Term, current. Display-model terminal. M17.
- tween (76). Discontinued. Skip.
- utorrent (74). Display-model fallback. Skip.
- vipmud (57). Niche. Later.
- voiceaccess (50). Current. Good-UIA, accept every notification. Later.
- webconferenceplugin (19). Unidentified. Skip.
- whatsapp_root (13). Current. Browse mode off by default. Tier C, a declaration.
- winamp (155). Legacy; remote memory. Skip.
- windowsinternal_composableshell_experiences_textinput_inputapp (448; also textinputhost). Emoji panel and clipboard history, current. IME candidate overlays, synthetic event queueing. Tier A after IME support.
- winword (169). Current. Object model through the document, three scripts, math setting on app focus. Tier D.
- wlmail (74). Discontinued. Skip.
- wwahost (43). Legacy host; its resolution rule is in M5's identity table. Skip as a module.
- zoom (16). Current. `requestEvents` for alerts from `zoom_acc_notify_wnd`. An event rule.

Counting mechanisms across the 78: class choice in about 50 modules, mostly one property overridden or a focus vetoed by a predicate on class name, control id, automation id or role, which are rules; the object-init hook in about 25, the same; event handlers in about 30, often to drop or redirect, which are hooks; scripts in about 20; good and bad UIA window hooks in 11, requested events in 2, sleep mode in 5, browse-mode default in 4, status bar providers in 6, raw UIA client calls in about 8 (the scoped finds), COM object models in 6 (tier D), in-process or memory reads in a handful (refused), process facts in about 10, mouse in 4 (later), config reads in about 8, a timer in 1 and a thread in 1 (refused). No module uses the registry or ships a settings panel.
