# Extensions: the design

The design of Verbatim's extension system, the equivalent of NVDA's app modules, add-ons and synth drivers, settled on 2026-10-22. It is implemented by M5 ([work plan](m5-extensions-work.md)) and, for native speech engines, M6 ([work plan](m6-native-synth-work.md)). D6 in the architecture is the decision of record; this document is its detail. Where a sentence carries a name and a date, it is a decision Dickson took and is not reopened without new evidence.

## 1. Terminology

- An **extension** is the installable unit: one `.vext` file, a zip holding an extension manifest, locale files and one or more module folders. NVDA's add-on.
- A **module** is one contribution inside an extension, of a **kind**: an **app module**, bound to one or more applications and active while such an application has an outpost, or a **synthesizer**, NVDA's synth driver. One extension may hold several modules; NVDA add-ons commonly ship several app modules. Global modules, NVDA's global plugins, and braille drivers are later kinds.
- A **built-in** is a module folder that ships with Verbatim under `extensions/builtin/` (planned), Verbatim's analogue of NVDA's `appModules/` directory. It is loaded by the same host as an installed module but is never bundled, listed, installed or disabled.
- A **declaration** is data in a module's manifest that the host merges into its own tables at load.
- A **rule** is a declaration of the form "match this, do that": a matcher and an action the host applies itself, without calling the module.
- A **hook** is a function a module exports and the host calls, on the thread that holds the event, with a deadline.
- A **capability** is a group of host functions a module imports. It is derived from the component's imports and never declared.
- The **application identity** of an outpost is the name a module claims to bind to it: the executable name, replaced for the hosts described in section 3.
- The **host** is the Verbatim code that loads modules and gives them their API; it runs in the outposts and in Core, never in the listener.
- The **adapter** is the engine-specific DLL a native synthesizer extension ships, implementing Verbatim's C ABI and loading the engine; the **synthesizer host** is `verbatim-synth-host`, one process per engine (D18).

## 2. Extensions, modules and built-ins

### 2.1 One folder layout

A module is always a folder containing `module.toml` and, unless the module is rules only, a component named after the module id:

```
calculator/
  module.toml
  calculator.wasm
```

The manifest names the component with its `component` key; the name is a convention. The two kinds of module differ only in what surrounds the folder:

- A built-in is that folder under `extensions/builtin/` (planned) and nothing else: no extension manifest, no locale files, because its strings live in Verbatim's own Fluent resources and its capabilities are read from the component's imports like everyone else's (Dickson, 2026-10-22).
- An installed extension is the `.vext` zip: a `manifest.toml` at the root with the extension's id, display name, version, API version and the list of module folders; a `locale/<lang>/*.ftl` folder for its strings, loaded with `verbatim-i18n`'s runtime loader with English as the fallback; and the module folders. Install extracts it to `extensions/<extension id>/` under the configuration root beside the executable, which the end-to-end suite stages fresh per run.

The loader has one code path for a module folder wherever it finds it, under `builtin/`, under an installed extension, or in the developer folder, so a developer can copy a module folder out of a built-in to work on it, or wrap it into a `.vext` to ship it, without changing a file inside the folder. The zip reader reuses the theme packager's limits (256 files, 16 MiB per file, 64 MiB unpacked).

### 2.2 Built-ins

Source lives in this repository under `extensions/builtin/<module>/`, one Cargo package per module with code, built by `cargo xtask build-extensions` into unpacked folders under `target/debug/extensions/builtin/`, beside the other deployed data folders, which `cargo xtask ci`, the end-to-end job and the `ci-arm64` job all run. Built-ins are not embedded into `verbatim.exe`, so that a folder stays editable and reloadable during development and the executable stays small.

Built-ins are not in the extensions list and cannot be disabled by the user (Dickson, 2026-10-22): they are part of Verbatim, and a user who hears Explorer behave oddly reports a Verbatim bug. With the developer setting on, `verbatim-inspect` and the Extensions dialog show them read-only with their derived capabilities and activity, so the layering of section 7 stays auditable. Their capabilities are granted without a prompt because they ship in the release, and enforced exactly as a third party's are.

### 2.3 Loading, the developer folder, hot reload and failures

A component is compiled once into a content-named file, and the process that runs it maps that file, never the source, so a source file can be overwritten while its module runs. Wasmtime trusts the code it maps, so compiled code is mapped only from where ordinary processes cannot write it or by a process it cannot harm: built-ins arrive compiled in the administrator-owned installation, an installed extension's compiled code is loaded only by that extension's own host (section 6), and the developer folder, watched only in developer mode, compiles at load ([security model](security-model.md), X5).

The developer folder holds unpacked module folders, is created if missing, and its modules sit above installed extensions in the ordering. Core watches it, and the built-in directory, only while developer mode is on, so no program can load a module without the install dialog by dropping it there, with directory-change notifications and a debounce, so that `cargo xtask build-extensions` writing over `target/debug/extensions/builtin/calculator/calculator.wasm` reloads the running development Verbatim. A reload validates the new version fully first (manifest, API version, rules against their subject and backend, imports against the world, compilation), rebuilds the merged rule tables, and tells each hosting outpost to instantiate the new version, run its load hook and swap between two events on the worker, dropping the old instance and its state. The watch covers the whole module folder, so a `module.toml` change reloads the same way, one path for any change; a folder appearing in the developer folder loads a new module and a folder removed unloads one. Speech says "Reloaded <module>", or "Reload of <module> failed" with the first line of the reason while the previous version keeps running; the full reason goes to the log. Installed extensions do not hot reload; install and remove are explicit operations on the same validate-then-swap path. Without the developer setting a changed built-in takes effect at the next start.

At startup every module loads independently; a failure never stops another module or Verbatim. A built-in failing is a defect or a damaged install, since the build validates built-ins against the world: it is logged as an error and spoken once as "Built-in module <name> failed to load", and Verbatim runs with core behaviour for that application. Installed extensions' failures are one spoken sentence, "<n> extensions failed to load", with each marked failed in the Extensions dialog with its reason and a partly loaded extension shown as such. The developer folder speaks each failure by module name. A duplicate module id fails the later one.

### 2.4 Secure mode

Secure mode (M8.1) loads only a machine-wide copy of the extensions the user allowlisted for secure screens, an allowlist of plain data and never whole folders, made by an elevated step into a folder the installer has made readable for the synthesizer host's container, as NVDA copies selected add-ons into its system configuration; it loads them read-only with installation, the developer folder and the console disabled (Dickson, 2026-10-22: synthesizers, and optionally app modules, must work at sign-in and in elevated contexts). The lock screen, the sign-in and credential prompts, the UAC consent prompt and Windows Security count as secure screens for extensions wherever they run, so a module claims them only if the user allowlisted it; their processes are listed in the loader. The copy is machine-wide, so the administrator who last made it chooses for every user's sign-in and UAC screens, which the copying step says.

## 3. Binding a module to an application

A module binds by application identity, which the supervisor resolves when an outpost launches, as a fixed core table and never through a module, since a module cannot run before the binding that loads it:

- the outpost's executable name, normally;
- for a console window, the console client's executable, which Windows names as the window's owner and which the outpost already attributes (`console_client`), following NVDA's per-object process rule;
- for `wwahost.exe`, the hosted application's name from the process's application user model id, and for `msedgewebview2.exe`, the parent process's executable, as NVDA's `getAppNameFromHost` does in `wwahost.py` and `msedgewebview2.py`.

Settings needs no special rule: its content elements already report `SystemSettings.exe` and run in their own outpost beside ApplicationFrameHost's frame. A console host's outpost loads a module claiming a client identity the first time it reports one. The listener learns a process's identity the same way, with one cached local image-name query per process id, so rules can be scoped before any outpost exists.

A module's `applications` list is NVDA's alias table written in the module's own manifest: `appModules/__init__.py` maps `calculatorapp` to `calculator`, `code - insiders` to `code` and `dbeaver` to `eclipse`, for executables renamed across Windows versions, insiders builds, bitness variants and forks. There is one module instance per outpost, as NVDA keeps one `AppModule` per process in its running table, so two processes of one application are two instances with separate state in both readers. The state a module keeps between hooks is its instance's own memory, which lives for the outpost's lifetime and is lost on reload or fault. An installed extension may not claim Verbatim's own executables, since Verbatim reads its own dialogs through an outpost.

## 4. The surface

Everything a module may declare, match, export or call in M5. Anything not here is refused at load, and an addition needs a use the list lacks. Each item exists because an NVDA module or a Verbatim rule uses it; the NVDA inventory behind the list is appendix A.

### 4.1 Declarations

- Extension identity: id, display name, version, API version (section 11). The extension is the unit of install, capabilities, version and ordering.
- Modules: id, kind (`app` or `synthesizer`), the component file.
- The applications an app module claims, by identity (section 3).
- Commands: id, default gestures, description, category, pass-through flag, speak-on-demand flag (NVDA `@script`). Module gestures are rebindable at run time with an open command id.
- Sleep mode and browse-mode default for the claimed applications (NVDA `sleepMode`, `disableBrowseModeByDefault`; browse mode is M7).
- Locale files for installed extensions; none for built-ins.

Not declared: capabilities (section 9), module settings and a key-value store (dropped for M5, Dickson 2026-10-22; no inventoried module needs them).

### 4.2 Rules

Every match-based declaration is a rule with a `subject`, so the manifest has one shape for "match this, do that" (Dickson, 2026-10-22):

- `window`: matched by window class, root window class and executable, before a backend is chosen. Actions: `backend` (NVDA's `isGoodUIAWindow` and `isBadUIAWindow`), `silent` focus and foreground, taken as any focus, cutting speech, with nothing said (Explorer's staging windows), `normalize-class` (NVDA's `windowClassMap`).
- `event`: matched by kind, window class and activity id in the acceptance filter. Actions: `accept`, which speaks the event regardless of the focused application and makes the supervisor start an outpost for the sender when it has none, since an accepted event needs one and nothing else does (Explorer's snap results; NVDA's `requestEvents`); `drop`.
- `node`: matched against the node's cached snapshot once the outpost has it, with a mandatory `backend`.

Window and event rules accept exact strings only and compile into the hash tables the listener and the outposts consult today; only node rules may use prefix or regular-expression matching. The subject fixes the legal properties and actions, checked at load.

Node matcher properties, all read from the cached snapshot or facts about the outpost, never fetched:

- Any backend: executable, normalized window class, role, states, name (exact, prefix or regular expression).
- UIA: automation id, UIA class name, control type, framework.
- MSAA: control id (`GetDlgCtrlID` on the node's window, NVDA's `windowControlID`, one new user32 call per MSAA snapshot recorded in the performance ledger), raw MSAA role.
- IA2 (M7): IA2 role, object attributes by key.
- Combinators: a list of values means any of them; `all`, `any`, `not`. A scope names which event kinds, or node creation, the rule applies to.

Every rule names its `backend`, and a rule naming a property of another backend is refused at load, so the backend is explicit (Dickson, 2026-10-22). A rule matches one node, so no condition spans backends; a fix needed under both is two rules. Every rule has a required `id`, unique within its module and validated at load, so the log, the provenance and the console name it as `<module>/<rule id>`, which survives reordering; the loader keeps each rule's manifest line for error messages. Every rule, of any subject, is scoped to the sender's application identity, which must be one the module claims, as NVDA's `isGoodUIAWindow` runs only for the module's own process.

Actions, one or more per rule:

- Set role; set name, value or description from a literal or from another property; clear name, value or description; apply a named text transform (strip ampersand, strip bidirectional marks, trim).
- Add or remove states; set the check state (unchecked, checked or mixed) or the expansion state (expanded or collapsed), each one field of the model, so two exclusive states can never hold at once (Dickson, 2026-10-28).
- Presentation: layout, hidden from navigation and announcement (NVDA `presType_layout`), or content; suppress position information.
- Drop the event; drop notifications for the node.
- Veto the focus event (NVDA `shouldAllowIAccessibleFocusEvent`).
- Redirect the focus to the parent, the first child, or a descendant by automation id (NVDA `focusRedirect`).

Conflicts (Dickson, 2026-10-22): within one module, rules apply in manifest order and the later wins a contested property, both recorded in the provenance; two window or event rules of one module naming the same exact class or activity id with different actions are refused at load with both ids named; two node rules of one module that set the same property of one node to different values log a warning naming both ids, once per pair per session, since their overlap is not decidable at load. Across layers the higher layer winning is the designed override (section 7). A drop from any layer stands and ends the event's processing.

### 4.3 Hooks

Functions a module exports; the host calls them on the thread holding the event, with a deadline (section 8).

- `on-load`, `on-unload`: the outpost's start and end.
- `on-event(node, event)`, registered per event kind and matcher, returning `pass`, `drop`, `replace(node)`, `rewrite(property changes)` or `adopt` (NVDA's `event_*` with `nextHandler`). `adopt` means the sender is the current focus's element: the outpost keeps the current node, announces nothing and follows the sender; it exists for the Windows Terminal twin rule (section 6).
- `on-node(node)`, returning property changes, when a rule cannot express the fix (NVDA `event_NVDAObject_init`, `chooseNVDAObjectOverlayClasses`).
- `status-bar(focus)`, returning a node or nothing (NVDA `_get_statusBar`).
- `on-command(command id, gesture, repeat count)` (NVDA `script_*`), resolved in Core by the ordering rule and run in the module's outpost, one Core-to-outpost-to-Core round trip per module command, since Core-side hooks may not read.
- A synthesizer module exports the `SynthDriver` contract instead (section 13).

Deferred past M5 (Dickson, 2026-10-22): an utterance hook (no NVDA app module rewrites speech, and it would run on every utterance), a settings-changed hook (modules read settings when they need them, as NVDA's do), and the timer and response completions of asynchronous imports (no M5 module uses one).

### 4.4 Imports

Functions a module calls, grouped by capability.

- **Node reading**, for an app module's own application, synchronous on the outpost worker: the node's own properties, including the backend-specific ones above and, for UIA, the runtime id; `parent`, `children`, `first-child`, `last-child`, `next-sibling`, `previous-sibling`; `find-descendant(matcher, depth limit)` and `find-ancestor(matcher)`, each one remote operation where the backend allows; `window()` for the node's window root; `text()` for a text control's whole value; `focus()` and `foreground()` for the outpost's current nodes. The shape is one for every backend; the host chooses remote operations or classic calls, each node carries its backend, and a find whose matcher names another backend's property returns an error.

  These imports are thin bindings over the outpost's own reading functions, never a second query implementation, so a module sees exactly what its outpost sees by construction (Dickson, 2026-10-22). Accessibility trees cross process boundaries: a frame host's tree contains the hosted application's elements, parent walks reach the desktop and every top-level window, a console window's text lives in `conhost.exe`. The outpost's readers stop at the boundary by the outpost's own resolved identity, as the focus walk does today; readers that lack the stop gain it once, for the outpost's own reads as much as for modules. So `parent()` answers none at the boundary, `find-descendant` skips a foreign subtree, and `window()` answers none for another application's window, with no call added, since the process id is a cached UIA property or a window-handle lookup under MSAA.
- **Output**, effects to Core, never awaited: `speak(message)`, `cancel-speech()`, `tone(hz, ms)`, `browseable-message(title, text)`. Enforced by attention: `cancel-speech` is honoured only while the module's application holds attention, and a module whose application does not speaks at queued priority, as D14 treats background events.
- **Commands**: `pass-through()` for a key the binding did not declare as passed; the repeat count arrives as an argument.
- **Process facts**, read-only from the supervisor: executable, path, product version, bitness, console client.
- **Settings**: a read-only view of every reader setting (Dickson, 2026-10-22).
- **Log**: `log(level, message)` under the module's id.

Deferred past M5: timer, fetch to named hosts, clipboard, mouse, and their completion hooks; a global module's reading of every application.

### 4.5 Refused

Raw UIA or COM, in-process helper calls, the display model, cross-process memory, threads, the registry, the file system, process creation, any read from Core-side hooks, every WASI interface (the extension world imports none: guests build for `wasm32-unknown-unknown` and are wrapped into components with no adapter, so a Rust guest carries no WASI import; the synthesizer world's permitted set is settled in M6), and, for installed extensions, a claim on Verbatim's own executables. Modules of other kinds that need native code, such as NVDA's modules that read another process's memory, are not supported by design.

### 4.6 Security rules of the surface

The surface's rules from the [security model](security-model.md), each checked at load or enforced by the host, with a test (Dickson, 2026-10-31):

- **Every navigation import stops at the application's boundary.** Every import that returns a node, `next-sibling`, `previous-sibling` and `children` included, returns none for a node outside the module's application, checked by the node's process in one place in the bindings, since a top-level window's siblings are other applications' windows and a window's children can belong to another process; `focus()` and `foreground()` answer none when the focus or the foreground is another application's.
- **Reserved gestures.** A module may not bind cancelling speech, sleep mode, the Verbatim menu, quitting, the screen curtain and its status, or, from M12, ending a remote session.
- **Bounded output.** `speak` and `browseable-message` have per-module rates and sizes, `tone` caps its duration and frequency, and `log` caps its line size and rate, since it reaches disk. `browseable-message` shows plain text, or HTML only after an allowlist sanitizer with no scripts, external resources or links that start programs.
- **Unforgeable node handles.** A node a module holds is a component-model resource owned by its instance, and `replace(node)` accepts only such a node.
- **No secrets in the settings view.** Remote's pairing keys and trusted certificates, and any future token, live outside the settings store.
- **Linear-time matchers.** Node rules' regular expressions use the `regex` crate with a size limit, and `find-descendant` is bounded by node count as well as depth.
- **Global modules' read boundary is part of their design**, which states which applications a global module may read, how that is shown at install, and whether installed extensions may provide one.
- **An exit is shown with what the module reads.** A future import that sends data out (fetch, clipboard writes, storage) is shown in one sentence with the applications the module reads.
- Accepted: a module may remove the protected state, where an application sets it wrongly; the provenance names the module that did.

## 5. An example manifest

An installed extension with two app modules, each item an NVDA fix verified in the pinned source (`explorer.py` for `isGoodUIAWindow` and the staging windows, `taskmgr.py`).

```toml
[extension]
id = "example.windows-fixes"
name = "windows-fixes-name"            # Fluent message id, from locale/
version = "1.2.0"
api = "0.1"
modules = ["explorer", "taskmgr"]
```

`explorer/module.toml`, rules only, so the folder has no component:

```toml
kind = "app"
applications = ["explorer.exe"]

[[rules]]
id = "shell-roots-are-uia"
subject = "window"
match = { root-class = ["Shell_TrayWnd", "Shell_InputSwitchTopLevelWindow",
                        "XamlExplorerHostIslandWindow", "TopLevelWindowForOverflowXamlIsland"],
          class-not = ["Start"] }
action = { backend = "uia" }

[[rules]]
id = "staging-windows-are-silent"
subject = "window"
match = { class = ["ForegroundStaging", "LauncherTipWnd", "ApplicationManager_DesktopShellWindow"] }
action = { silent = ["focus", "foreground"] }

[[rules]]
id = "snap-results-are-spoken"
subject = "event"
match = { kind = "notification", activity-id = "Windows.Shell.SnapComponent.SnapHotKeyResults" }
action = "accept"
```

`taskmgr/module.toml`, one window rule and one node rule:

```toml
kind = "app"
applications = ["taskmgr.exe"]

[[rules]]
id = "task-list-is-msaa"
subject = "window"
match = { class = ["SysListView32"] }
action = { backend = "msaa" }

[[rules]]
id = "row-icon-is-layout"
subject = "node"
backend = "uia"
match = { automation-id = ["TmRowIcon"] }
action = { presentation = "layout" }
```

A module with code adds `component = "calculator.wasm"`, its hook registrations, and its commands:

```toml
kind = "app"
applications = ["calculatorapp.exe"]
component = "calculator.wasm"

[[hooks]]
hook = "on-event"
kinds = ["name-change", "notification"]
backend = "uia"

[[commands]]
id = "calculator-result"
gestures = ["enter", "numpadEnter", "escape", "delete", "numpadDelete"]
description = "calculator-result-description"
pass-through = true
```

A built-in's `module.toml` is identical; only the extension manifest and the locale folder are absent.

## 6. Where modules run

The host runs in these processes:

- **An installed extension's own host** runs that extension's app modules, one process per installed extension, started when an outpost starts for an application it claims, shared by all of them, and ended when none is running ([security model](security-model.md), X4 and section 6.9). It runs under the strictest restricted token at untrusted integrity, with no window-manager calls, one process in its job, a memory cap and a processor rate cap, and makes no call into any application: its hooks' reads are requests to the claiming outpost over a pipe, and the outpost answers them only within the module's application (section 4.6) and its derived capabilities, judged by the pipe the request arrived on, never by a module id the host sends. A hook run this way costs a round trip and two wake-ups on the event's path, which M5 measures.
- **Outposts** host each built-in app module's element-level code: rules applied where the outpost emits a snapshot, and hooks that see a node with its cached properties and can read further through the worker. The instance is created when the outpost starts for an application the module claims and ends with the outpost, which is NVDA's per-process `AppModule` lifetime. Only outposts for applications with a module load a runtime at all, so a plain application costs nothing new, and the outpost's existing deadline, watchdog and kill-and-replace ladder cover module faults.
- **Core** resolves commands by the ordering rule, executes modules' output effects, and hosts the call console (section 12). It runs no hook that reads.
- **The listener** runs no module code, ever: its callbacks may not block (D13), its judgement is per application on reading threads, and loading a runtime there would cost every application's memory for the few that have modules. It applies window and event rules as data from the merged tables.
- **The synthesizer host** (D18) hosts synthesizer modules (section 13).
- **The injection helper** (M7) runs no module code.

Core's reducer stays pure: no module hook runs on the reducer thread. A module's speech is an ordinary effect, and a module command is an input Core recorded, so the flight recorder's replay needs no module.

The Windows Terminal second identity ([design](windows-terminal-twin-focus.md)) is a module, not core code (Dickson, 2026-10-22: application-specific logic is not hard-coded in Verbatim). Its `on-event` hook in the Terminal built-in, registered for UIA focus events on `TermControl`, runs in Terminal's outpost after the listener's read; the event reaches it only after the listener's `CompareElements` judgement has found the sender and the current focus different, so no same-element import is needed. It reads the runtime id, class name, control type and framework from the node, compares `node.window()` with `focus().window()`, and answers `drop` for a 14 sender twinning a current 42 focus or `adopt` for a 42 sender twinning a current 14 focus. The extra cost falls only on the rare twin, measured in the design. The evidence the roadmap asks for, that the runtime id's last part is never reused within a process, is collected first.

Firefox and Chromium read through IA2 out of process first (M7); a browser's module runs in its outpost like any other and never inside the browser.

## 7. Ordering, provenance and conflicts

Every module that claims an application loads into that application's outpost, one instance each, in one explicit order: installed extensions first, in the order of the extensions list, which is installation order with the newest highest until reordering exists; then built-ins, so an installed extension's module overrides Verbatim's own for the same application by design; then core behaviour. Two modules of one extension on one application are ordered by their position in its manifest. NVDA's effective order, from the pinned source, is that the more general plugin wins and the order among plugins is undefined; Verbatim's is explicit and shown in the Extensions dialog.

Within one event (Dickson, 2026-10-22): every layer's rules apply first, lowest layer first so the highest layer's action wins a contested property, with every layer recorded in the provenance; then hooks run from the highest layer down, each seeing the node as the rules and the hooks above left it, `pass` continuing the chain and `drop`, `adopt` or `replace` ending it, `on-node` rewrites accumulating in that order. This is NVDA's `nextHandler` chain with the order made explicit and the layer that stopped it logged. So in both the highest layer has the final say, its rules applying last and its hooks running first, and a drop is never undone by a later layer (Dickson, 2026-10-28). A layer's rule matchers see the node as the rules of the layers below it left it. For commands, the highest layer that binds the gesture gets it; the user's own gesture map sits above every layer once it exists.

Every property the reducer speaks has one base source and may be overridden by layers. The outpost records for each overridden field which layer and rule set it, and the focus dump shows each property as its value and its source: "role: edit, set by notepad/goto-line-is-edit, was dialog from UIA". Every dropped event names the layer and rule that dropped it in the log. Conflicts are shown, never hidden: provenance names every layer that set a field, the log names the dropping layer, and the Extensions dialog's command list marks the winner of a contested gesture. In M5 the user's lever is uninstalling; reordering comes later.

## 8. Performance

### 8.1 Declarations before hooks

A module that only declares costs the same per event as the constant tables cost today: the host merges every active module's rules into the tables it consults already, once at load, keyed by identity, so a lookup stays one hash probe. Node rules are checked only for events from the module's own application and only against properties the outpost already has in its cache. Hooks are called only where a module registered interest by event kind and matcher. Of NVDA's 78 app modules, about fifty need only rules (appendix A).

So that rules can match and `verbatim-inspect` can show them, the snapshot's details always carry the UIA class name, automation id and framework, which the outposts already cache, the window class of the node's window from the window cache, and for MSAA nodes the control id (Dickson, 2026-10-22: the information is useful on its own). Core keeps the application identity per outpost beside the attention record.

### 8.2 What a hook costs

The call from host to guest is tens of nanoseconds and marshalling a snapshot is under a microsecond; the cost that matters is a thread hop, which the communication design counts in tens of microseconds. So hooks run on the thread that already holds the event: in an outpost on its worker, inside the worker's existing deadline and watchdog. Per event, the time inside module code (guest time and host-call time, with the reads made) is a stage of the latency timeline, so a slow event is attributed to the module rather than to the outpost read; a rules-only module adds no stage. The host records per-module hook counts and times, visible in `verbatim-inspect`, and the call ledger carries two permanent rows ratcheted against `mockapp`: a rules-only module leaves every count unchanged, and a module with one descendant read adds exactly one UIA call and nothing else.

### 8.3 Budgets and faults

NVDA's `watchdog.py` has no per-handler budget: module code runs on its main thread, the watchdog waits half a second and then up to ten seconds, then cancels only COM calls and `SendMessage`; pure Python is never interrupted, and the whole reader freezes meanwhile. Verbatim uses wasmtime's epoch interruption and a per-store resource limiter for linear memory, tables and stack, inside the job object cap the outpost already has.

There is no periodic ticker, since it would wake a thread every millisecond. The deadline is the current epoch plus one, and the outpost's existing watchdog alarm is re-armed at hook entry and increments the epoch once if the hook overruns, so no wake-up occurs unless a hook overruns; the resolution is the alarm's, about a millisecond, and the re-arming is a timer call on the event path the ledger states. Two bounds guard two things:

- The total bound is the worker's remaining handling deadline for the event, ten seconds, chosen in `worker.rs` because starting applications take two or three seconds to answer a read that then succeeds. It is enforced by refusing a read once spent, so a slow application trips a module only where it would trip the outpost.
- The compute bound is the epoch deadline, which the host extends by each read's duration when the read returns, so it measures the module's own execution only and a slow read cannot trip it. The epoch is wall-clock, so time inside a read would otherwise drain it (fuel would not count it at all), but the deadline check runs only in guest code, so a read in progress is bounded by the outpost's per-call deadline and watchdog ladder, and the trap lands at the first guest check after the read returns. Its value is set from the guest-time distribution the latency stage records once the first-party modules have run through the end-to-end suite, at a generous multiple of the slowest legitimate hook; until then it is deliberately loose, because the total bound still catches a looping module.

On a trap: the hook counts as `pass` for an event and "no change" for a node, so the user hears what the outpost had; the instance is discarded and re-created from the pre-linked form, its state lost; the fault is logged with module, hook, guest time and host-call time; repeated faults in a short window, counting only faults that were mostly guest time, unload the module for the outpost's lifetime with one report to the user, so a module is not unloaded for an application's slowness.

## 9. Capabilities and what the user sees

### 9.1 The model

What is protected, in order: the content Verbatim reads, which is everything on the user's screen; the user's keystrokes; the machine's files, network and processes; and Verbatim's availability, since a hung screen reader leaves a blind user without a computer. Against a malicious author, a compromised extension and, most often, a buggy one. The model makes what a module can reach visible and limitable; it cannot make a trusted module safe, and until signing and a store exist, installing an extension is trusting its author, which the install dialog says.

Isolation comes from Wasm: memory safety, no system calls, only the imports the host links. A component's binary lists the interfaces it imports with their versions, and the host links only the granted ones; anything else is absent, and the component fails to instantiate rather than reaching an ungranted function. Native code is never loaded into Core or an outpost; it exists only in the synthesizer host (section 13).

### 9.2 Derived, never declared

Capabilities are derived from the component's imports for built-ins and installed extensions alike (Dickson, 2026-10-22). Browsers and phones need permission manifests because JavaScript and phone apps reach privileged APIs dynamically and no inspection of the package can enumerate them; a Wasm component's import list is a complete, exact statement of every host function the code can ever call, produced by the compiler. A declaration would be a second copy that must equal the first. A capability with a parameter the import cannot carry, later the network host list, gets a manifest entry for the parameter only, and the import still decides whether the capability exists. Scope is expressed as which interface is imported. The packaging tool prints the derived list and any refusal before producing a `.vext`, which is where an author learns that a dependency pulled in an import.

### 9.3 Shown and silent

The install dialog and the Extensions dialog show only sentences that name something of the user's the module reaches or code outside Verbatim's control, as Chrome lists only permissions that carry a warning:

- Runs native code in a restricted process of its own, and receives all the text Verbatim speaks. The one warning, shown first.
- Reads the controls and text of the named applications. Always shown for an app module.
- Runs its own code when something happens in the named applications. Derived from exported hooks.
- Changes how Verbatim reads the named applications with N rules, with no code of its own. For a rules-only module.
- Adds keyboard commands in the named applications, one command and its default binding per line.
- Provides a speech synthesizer, named.
- Needs files you add yourself, named.
- Later: connects to the internet with the host list; reads and changes the clipboard; moves the mouse pointer; reads every application.

Silent but enforced: speech and sounds, the log, reading Verbatim's settings, process facts. The Extensions dialog's details view has a "show everything" line for them, and developer mode shows the full list for built-ins, which is how Verbatim checks that its own modules import no more than they need. Refused imports stop installation with the import named: "This extension cannot be installed: it needs wasi:sockets, which Verbatim does not provide to extensions."

Installing is a request to the running Verbatim, as NVDA does it: the `.vext` file type is registered to a small helper executable, as NVDA registers `.nvda-addon` to `nvda_slave.exe addons_installAddonPackage`, which sends the path over the control plane and exits, or shows a message box that Verbatim must be running when nothing answers. `verbatim.exe` has no install flag, so its single-instance takeover is never involved. The Extensions dialog in the Verbatim menu lists each installed extension with its version and status, shows the description and the sentences above with the module's activity counts, and uninstalls with a confirmation. No store, updates or reordering in M5.

## 10. The runtime and the guest workflow

### 10.1 wasmtime

The runtime is wasmtime with the component model (Dickson, 2026-10-22), reached by a requirements-driven comparison rather than by default: Windows x64 and ARM64, the component model and WIT, interruption of running code, memory limits, precompiled artifacts. wasmer dropped its Windows native compilers, wasmi and WAMR have no component model, WasmEdge is x64 only. The alternative was a host-defined ABI over core modules, which would admit wasmi but give up typed bindings generated from one definition for every guest language, versions carried in the binary and generated documentation. wasmtime is pinned to its long-term-support line, Cranelift compiles on both architectures, and the Pulley interpreter, an architecture-neutral bytecode interpreter inside wasmtime with the same API and interruption, is switched on for ARM64 only if the `ci-arm64` ratification or a Cranelift fault requires it. Winch is excluded because it cannot be epoch-interrupted. `verbatim-ext` is the only crate that names the runtime, as D6 asks; `verbatim-ext-api`, which holds the WIT files and the manifest and rule types, is platform neutral and on xtask's neutral list.

### 10.2 Rust guests

A guest crate uses `wit-bindgen` against the WIT in `verbatim-ext-api`, implements the generated export trait, and is type checked by ordinary compilation with rust-analyzer completion; `cargo doc` carries the WIT comments. Guests live in an `extensions/` (planned) Cargo workspace separate from the host workspace, so the platform-neutral check and `workspace-hack` never see a Wasm target, and build for `wasm32-unknown-unknown`, wrapped into components by `wasm-tools component new` with no WASI adapter, so they carry no WASI import. `cargo xtask build-extensions` builds, validates against the world and places each component beside its manifest; this is a named target, a listed exception to the coding standard that builds use the host's architecture. Any toolchain producing a component for the world works the same way, which is the test that other languages can be used. The guest SDK crate carries its own permissive licence so an extension's licence is the author's choice.

Scripted modules in TypeScript or JavaScript are not M5 (Dickson, 2026-10-22). JavaScript is never compiled to Wasm; the serverless providers compile an engine to Wasm and interpret the script inside it, and the engine's code and heap would be carried by every outpost hosting a scripted module. The QuickJS engine component, TypeScript erasure and the generated declarations move to M10 with the console, whose single Core instance pays the engine cost once; the API does not change, since an interpreter component is a component of the same world.

### 10.3 Testing

A module's decisions are functions over the generated types, unit tested in its own crate on the host with `cargo test`. Hooks that read or speak are integration tests in the module's folder, using the `verbatim-ext` test helper as a library against a fake tree, run by `cargo xtask build-extensions`. `verbatim-ext`'s own tests prove only the surface: round trips of every import and export, rule compilation per subject, budget interruption, memory fault containment, reload and failed reload, never application logic. End-to-end scenarios cover the real application, with spoken expectations taken from NVDA captures by the `/port` protocol.

## 11. Versioning

A WIT package carries a semantic version, and every interface a component imports is named with it in the binary. wasmtime's linker resolves an import by exact name first and otherwise by track: for major 0 the minor is the track, for major 1 and up the major is. Several tracks live side by side in one linker, so the host keeps the previous track for a deprecation period. Additions are marked `@since`, removals `@deprecated`, experiments `@unstable`, and `wasm-tools component semver-check` proves in CI that a new API version is a compatible evolution of the old.

The manifest carries the extension's own `version`, the `api` version the module was built against, and `verbatim-minimum` and `verbatim-tested`, mirroring NVDA's `minimumNVDAVersion` and `lastTestedNVDAVersion` with NVDA's rule and user override for an untested but otherwise supported extension. During M5 and the porting track the package is `verbatim:ext@0.x`, every minor a track, additions within a track `@since`-gated. At install the host reads the component's imports and refuses one that names a track it does not offer, with both versions named; the manifest's `api` must agree with the binary's imports. The binary is the authority; the manifest field is for people.

## 12. The console

In M5, `verbatim-inspect call` invokes any function of the world by name with WAVE-encoded arguments through the host's dynamic call path, with no interpreter and no variables; it inspects the focus, walks the tree and speaks from the command line, and it is the smoke test that every interface is callable by hand.

In M10, the interpreter console: the QuickJS component loaded in developer mode with every capability, running in Core on its own console thread, never the reducer thread and never in an outpost, with node reads as requests to the owning outpost's readers, synchronous from the script's view, so console snippets paste into modules unchanged. NVDA's console runs each line on its main thread with the watchdog disabled, so a long evaluation freezes NVDA; Verbatim's gets an epoch budget of seconds and Control+C in `verbatim-inspect` cancels it. Its snapshot variables follow NVDA's (`focus`, `focusAnc`, `fg`, `caretPos`, `review`, `nav`) with `outposts`, `attention`, `state` and `recorder` added; its rule functions test a rule against a node with the loader's validation, list the nodes a matcher selects, explain a node's provenance, and install session-scoped rules tagged as the console's; it traces events with their acceptance decisions and calls a module's hook by hand. Modification functions are not in its first version (Dickson, 2026-10-22), apart from installing session rules; when they come, a property override is a session rule and a behaviour change is an injected input recorded in the flight recorder, never a direct write.

## 13. Synthesizers and native code

### 13.1 Wasm synthesizers

The `verbatim:synth` world is the `SynthDriver` trait of `verbatim-speech` in WIT: `describe`, `set-setting`, `speak(sequence)` with the sink as the imports push PCM, mark reached and is cancelled. A Wasm engine, such as eSpeak NG compiled from its source, is one component that runs on x64 and ARM64 unchanged, in the synthesizer host for the machine's architecture (`verbatim-synth-host --wasm`), which wraps it as a `SynthDriver` so that from the pipe upward Core cannot tell it from a native or built-in engine. It sees only its preopened bundle folder, traps without killing the host (re-instantiated from the pre-linked form), is interrupted by the epoch when it stops producing, and is bounded by store limits. The tone synthesizer that proves the world lands in M6 beside the test adapter.

### 13.2 Native synthesizers

Wasm modules never call DLLs: a component has no foreign-function interface, by design. Native code is a property of one kind, the native synthesizer, and later the native braille driver. Three facts force it for Eloquence: the IBM engine is a proprietary 32-bit `ECI.DLL` with no source, so it cannot be compiled to Wasm; no 64-bit process can load a 32-bit DLL, and a native ARM64 process cannot load an x64 one; and its files cannot be distributed. The existing `eloquence_64` NVDA add-on confirms the shape: it ships no 64-bit engine and runs the 32-bit DLL in a 32-bit helper over a named pipe.

The mechanism is general and the host has no engine-specific code:

- The ABI, `verbatim_synth_v1`, is the `SynthDriver` trait spelled as C exports: version, create, destroy, describe, set a setting, speak a sequence through PCM, mark and is-cancelled callbacks, cancel, with every string a pointer and a length and every buffer owned by its allocator. A first-party adapter crate supplies the export boilerplate, so an adapter author implements `SynthDriver` in Rust in a `cdylib` with the engine calls inside.
- A native synthesizer module's manifest names one adapter library per architecture (`x86`, `x64`, `arm64`), named `<engine>-<arch>.dll`, and a `user-files` list of engine files the user must supply because they cannot be distributed; the Extensions dialog shows "add the engine files" while any is missing.
- The host for the declared architecture loads only the adapter, which loads the engine, and speaks the D18 protocol to Core unchanged; Core's mixer plays and resamples the PCM, so the host renders nothing. The x86 host is a cross-compile for `i686-pc-windows-msvc` shipped with every install, a reduced build carrying only the adapter loader and the protocol, not eSpeak NG or OneCore (Dickson, 2026-10-22), so no 32-bit C build is needed; it exists for as long as Windows 11 emulates x86 on x64 and ARM64. An x64 adapter on an ARM64 machine is unsupported until an x64 cross-build is added, which Eloquence does not need.
- Eloquence's adapter is first-party by location only: a Rust `cdylib` written from IBM's documented SDK, since the add-on has no licence to port from, mapping the engine's parameters to settings descriptors, inserting an index per mark, delivering 16-bit mono 11025 Hz buffers to the PCM callback and stopping on cancellation at the next callback. Its bundle in this repository holds an empty engine folder for `ECI.DLL`, `ECI.INI` and the voice files. OpenEVV, the MIT x64 reimplementation, is US English only and could be bundled with an x64 adapter later.

Guidance to engine authors: compile to Wasm when source exists; the native adapter is for engines that exist only as binaries.

### 13.3 The container

The synthesizer host runs in an AppContainer (M6): a process token with a package identity and the user's own rights stripped, Low integrity, no network, no registry writes, no access to other processes or windows, files only where the launching program grants the container's identity access. The contained launch in `verbatim-process` sets read and execute access entries for "all restricted application packages" on the host executable's folder and the bundle folder at launch, and an engine that must write gets a scratch folder of its own, since Core owns both; the installer later only avoids the repeat. The pipe to Core is inherited by handle, which keeps working because access is checked when a handle is opened, not on each use, which is the basis Chromium's renderers run on below Low integrity. The job object with its memory cap and kill-on-close stays around the container, because a container does not bound CPU or memory and cannot interrupt a hung native call.

The sandbox level is per hosted engine, since D18 runs one per process, and each engine declares the strictest level it is known to work under: eSpeak NG, which only reads files, can take the less privileged container or a restricted token at Untrusted integrity; OneCore needs WinRT activation and so the ordinary AppContainer at most, since Store apps call it from one and the less privileged container blocks COM activation without a capability; native adapters start at the less privileged container (Dickson, 2026-10-22: LPAC first, and a fall-back to the ordinary container only with the failing call named and recorded as a workaround). M6 establishes the levels by running the three.

Low integrity does not hinder the host at the sign-in screen or beside an elevated application: the host writes only to the inherited pipe and needs no window, desktop, clipboard, registry or UIA access, and a system-account parent can create container children. What changes at sign-in is the missing user profile, which section 2.4's machine-wide copy answers. Outposts cannot run in an AppContainer, because Windows denies a container process UIA and MSAA client access to other processes; they get restricted tokens, the job object and mitigations, and an app module works in a secure context wherever its outpost does. Braille displays will reuse the whole pattern with a `verbatim_braille_v1` ABI and a braille host; no other kind gets native code.

## 14. Porting NVDA's app modules

The porting track runs from M5 onward. The order is chosen so that each module can be tested on the VM or by Dickson, the software is current, and the module needs only the surface above. Dickson's daily applications (2026-10-22) are Windows Terminal, Firefox, Explorer, VS Code and Thunderbird; Firefox has no NVDA app module, since its support is NVDA's core and Verbatim's browse mode, and VS Code and Thunderbird are web-content hosts whose modules wait for M7.

In M5, every Windows inbox application: Explorer (the staging windows, the shell roots and the snap results as rules; the desktop-icon duplicate-focus veto, the tab-switch announcement and the taskbar and context-menu name fixes as hooks), Windows Terminal (the twin rule), Notepad (the status bar provider and the go-to-line role), Calculator (the display-updated notification read, the pass-through result keys), Task Manager (two rules, proven by `mockapp` until UIAccess lets it be read end to end), Settings (the volume meter is not a progress bar), Start and search (the bad-UIA rule for the Chromium render widget, the suggestion count name, the search field description; the ancestry-override part waits), Quick Settings and Open With. Sleep mode becomes a core per-application list toggled by NVDA's key and seeded with the five executables NVDA's sleep-only modules name, rather than five modules.

After M5, on the track: foobar2000 and Poedit, whose value is commands; Audacity; Notepad++, after a core Scintilla text provider in the outpost that reads the control by its window messages and a buffer allocated in the target process, as NVDA's `scintilla.py` does, since Scintilla exposes no usable UIA or MSAA text; the emoji panel after IME candidate support; lock screen and logon after M8.1; VS Code, Thunderbird and the WebView2 hosts after M7; Word, Excel, Outlook, PowerPoint, LibreOffice and Visual Studio after the object-model bridge; Eclipse after M13. Skipped, with the reason in appendix A: modules for discontinued software, display-model terminals until M14, and modules that read another process's memory.

A module is done when its NVDA behaviours are captured on the VM with the pinned build, one end-to-end scenario per behaviour asserts the captured utterance, its own tests pass, and the parity ledger lists each behaviour with the scenario that pins it.

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
- eclipse (208; also DBeaver, Spring Tool Suite, Zend). Current. Focus veto on unfocused tree items, an editable-text-with-suggestions overlay with three scripts. After M13.
- egui (16). Probably an older ESET GUI. Skip.
- esysuite (17; also esybraille). Eurobraille suite. Sleep only; the core sleep list.
- excel (144). Current. Good-UIA by version, overlays with a reparenting override, a focus redirect that polls, display-model edit box. Tier D.
- explorer (551). Current. Ten overlays, init hook naming, focus drops for staging windows and pending menus, tab-switch announcement, the snap notification, good-UIA roots, status bar, product info. Tier A, first.
- fastlogentry (25). Niche ham-radio logger; display model. Skip.
- foobar2000 (210). Current. Caches the status bar on focus; three scripts speak elapsed, remaining and total time. Tier A, on the track.
- hxmail (57; also commsapps, hxoutlook). Mail and Calendar, ended 2024. Skip.
- instantbird (25). Discontinued. Skip.
- itunes (123). Legacy. Skip.
- javaw (53). Host resolution for Java applications by parent process and command line. Tier E, after M13.
- kindle (464). Kindle for PC 1.x book view, IA2. Custom browse-mode interceptor. Tier C.
- lockapp (106). Windows lock screen, current. Overlays, input capture, review position save and restore. After M8.1.
- lockapphost (15). Legacy. Skip.
- logonui (129). Windows logon, current. Overlays, focus re-queue, password field name. After M8.1.
- loudtalks (42). Renamed Zello. Skip.
- lync (99). Skype for Business, discontinued. Skip.
- microsoftedge (20), microsoftedgecp (32). EdgeHTML, discontinued. Skip.
- mintty (7). Current; subclasses putty with its window class. Display model; M14.
- miranda32 (282; also miranda64). Custom window messages with remote memory. Skip.
- mirc (41). Current. Display-model live text. M14.
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
- putty (34). Current. Display-model terminal. M14.
- searchui (98; also searchapp, searchhost). Start and search, current. Bad-UIA for the Chromium render widget, overlays with ancestry overrides, suggestion count name. Tier A, partly later.
- securecrt (31). Current. Display-model terminal. M14.
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
- ttermpro (27). Tera Term, current. Display-model terminal. M14.
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
