# M5: the decisions Dickson owns, in order

Each decision needs the ones above it settled first. Once all are answered, Claude restates the design (the work plan's step 0b): the host's design, the states-and-rules tables, then one adversarial review. After that a fresh session can build step 1. Decisions outside Dickson's areas, such as the epoch strategy and the compute bound, are Claude's and are recorded in the design.

## A. Process and thread layout

1. **Where built-ins run.**
   - Options: in the outposts and Core (today's D6), or as one internal extension in a host process of its own.
   - Recommendation: one host. No Wasm runtime would remain in Core or any outpost, and the hook path becomes one path for every module.
   - Everything below depends on this.
2. **One host program or two.** The extension host and `verbatim-synth-host` (D18) could be one program or two.
   - Recommendation: one program. A synthesizer is a module kind, and M6's Wasm tone synthesizer already runs in the synthesizer host.
3. **Threads in a host.**
   - Recommendation: one thread per outpost the host serves, plus one for global modules.
   - Why: a hook waiting on a hung application's read, for up to 10 seconds, then never holds up another application's hooks.
4. **Routes.**
   - Recommendation: reads go from a host straight to the claiming outpost, over a pipe the supervisor makes. Speech, logs and every action that depends on attention go through Core.
   - This is also a trust boundary: the outpost judges a request by the pipe it arrived on.
5. **The built-in host's lifetime.** The taskbar and global modules make it needed almost always.
   - Options: start it with Verbatim and keep it running, or start it on demand.
   - Recommendation: start it with Verbatim.

## B. Scope

6. **Global modules in M5.** The tray list waits for a choice in the list dialog and then clicks the item. M5's surface has no prompt with a later answer, no click and no clock.
   - Option (a): add three narrow imports to M5: a list prompt answered through a later call, a click on a node the module holds, and a clock. Every press of Verbatim+F11 then costs a round trip to the host and the host's reads of Explorer.
   - Option (b): keep the tray list, taskbar list and time in core code until M13, reversing the 2026-10-31 decision.
   - Recommendation: (a), unless you would rather keep M5 smaller.

## C. Public contract

7. **The M5 world.** `extensions.md` section 4 is pinned. Decision 6(a) adds three imports to it. Confirm the surface is otherwise complete for the inbox built-ins listed in M5.

## D. What users hear

8. **The breaker for built-ins.** Today three failures in a minute turn off the whole extension.
   - Recommendation: count per module for built-ins.
   - Speech for a Calculator hook that traps three times in a minute:
     - Per module: "Built-in module Calculator was turned off after repeated failures". Explorer and Terminal keep working.
     - Per extension: "Built-in modules were turned off after repeated failures". Every inbox application falls back to core behaviour.
9. **The helper's message box** when Verbatim is not running: "Cannot install the Verbatim extension from {path}. Verbatim must be running to install extensions." This matches NVDA's `nvda_slave.pyw`, lines 110 to 117.

## E. Performance budgets

10. **The cost of a hook.** Under decision 1 every built-in hook costs one round trip, about tens of microseconds, and two wake-ups, plus two wake-ups per read. This lands on:
    - every Terminal `TermControl` focus event (the twin rule);
    - Explorer's code hooks;
    - every Calculator display update.

    Decide whether that is accepted, to be measured in step 4.
11. **The built-in host's memory.** It has none of its own, while every outpost has a memory cap. Recommendation: set one in step 0b from a measured start-up figure.

## F. Trust boundary

12. **Built-ins' compiled code.** X5 says built-ins arrive compiled in the administrator-owned installation, which a portable copy does not have.
    - Recommendation: until M8.7, the built-in host compiles built-ins at load into the cache beside the executable. A planted file there reaches only the contained built-in host.

## Not blocking M5

- **The order of M8.6 and M8.7.** M8.6's UIAccess needs a signed copy in a trusted location (R5), which is an installed copy, yet M8.7, which builds the installed copy, comes after it.
- **Roaming or Local** for the installed copy's per-user files. This is decided in M8.7.

## Answers so far (Dickson, 2026-11-09)

- 1 agreed: built-ins run as one internal extension in a host of their own.
- 2 open: the built-in engines (eSpeak NG, OneCore) stay native engines, each in its own process, as D18 has them. The question left is whether that process and an extension's host are one executable with two modes or two executables.
- 3 agreed, generalised to lanes: a host has one thread per lane, a lane being anything that can wait on its own. That means each outpost it serves, each global module, the extension's speech-text stages (one lane, called in chain order), each synthesizer, each braille driver, and each recogniser. Calls within a lane are serial. M5 builds the outpost and global-module lanes only.
- 4 agreed: a guest's read import becomes a read request sent from the host to the outpost.
- 5 agreed: the built-in host always runs.
- 6: must be fixed in M5. The API proposal is below. The tray list is designed against the planned MSAA reading of the taskbar's `Windows.UI.Input.InputSite.WindowClass` window (roadmap M5), not today's UIA code, which does not work on build 26200.
- 8 agreed: per module.
- 9, 11 and 12 agreed.
- Roadmap: M8.6 is now the installer and M8.7 UIAccess (working tree, uncommitted).

## Decision 6: the proposed additions to the M5 world

For global modules only, each gated in Core:

- **Declaration** `reads = ["explorer.exe"]` in a global module's manifest: the applications it may read. The install dialog shows "Reads the controls and text of File Explorer". This is the global modules' read boundary, which `extensions.md` section 4.6 requires.
- **Import** `find-window(application, class-path, backend) -> option<node>`: a top-level window of the named application whose class is the first entry, then each later entry a descendant window class. It returns a node in that application's outpost, read through the named backend. Navigation from the node then stops at that application's boundary, as for app modules. It is refused for an application not in `reads`.
- **Import** `clock() -> {time, date}`: the local time without seconds and the long date, formatted by the user's regional settings, as `crates/verbatim-app/src/datetime.rs` formats them today. To check in step 4: whether the host's less privileged container can read the regional settings. If it cannot, Core formats them.
- **Import** `list-prompt(title, label, items, buttons) -> prompt-id`: Core shows the M3 list dialog. It is allowed only during an `on-command` hook, with one prompt per module at a time.
- **Hook** `on-prompt-answer(prompt-id, answer)`: the answer is either a chosen item index with a button index, or cancelled.
- **Import** `click(node, kind) -> result<_, gone | unclickable>`: the kind is left, left double or right. Core asks the node's outpost where the element is now, then moves the pointer and clicks. This works from a held node, so it needs no re-matching by name. It is allowed only during `on-prompt-answer` or `on-command`.
- **Node handles kept between hooks:** a host reports the nodes it holds, as Core does, so the outpost keeps their elements alive until the answer arrives.

So Verbatim+F11 works like this:

1. `on-command` runs `find-window("explorer.exe", ["Shell_TrayWnd", "Windows.UI.Input.InputSite.WindowClass"], msaa)`.
2. It reads `children()` and their names, keeping the nodes.
3. It calls `list-prompt`.
4. When the answer comes, `on-prompt-answer` calls `click` on the chosen node, and speaks the existing "gone" or "can't be clicked" message when the click fails.

## Round 3 (Dickson, 2026-11-09)

- 10 agreed: the hook's round trip and wake-ups are accepted, measured in step 4.
- Clock formatting does not belong in Core. If the host's less privileged container cannot read the regional settings, the built-in host falls back to the ordinary AppContainer, with the failing call named, as `extensions.md` section 13.3 allows for engines.
- No second prompt while one waits. Proposed rule, to confirm:
  - `list-prompt` while the module's prompt is open fails with `already-open`, and Core brings the open dialog to the front.
  - A press within the repeat interval (the double press for the taskbar) uses `replace-prompt(prompt-id, spec)`, which changes the open dialog's list in place.
- The click gate is to be decided (reason given in chat).
- Synthesizer placement is restated in chat.
- The surface walkthrough goes one section at a time. Section 1, the declarations, found these gaps:
  - no `global` module kind;
  - no author or description, which the install dialog shows;
  - no matcher on hook registrations;
  - no rule for when an app module's command is active;
  - the browse-mode default has no effect until M7;
  - sleep-mode declarations must feed the core sleep list;
  - speak-on-demand depends on a speech mode Verbatim may not have.

## Round 4 (Dickson, 2026-11-09)

- Clicks and prompts have no gates for now. The rule of one prompt per module at a time stays.
- `verbatim-minimum` is dropped, since `api` covers it. Proposed: drop `verbatim-tested` too.
- App-module commands are active only while a claimed application is in front.
- Proposed, to replace the two-mode host: eSpeak NG and OneCore become built-in extensions holding native synthesizer modules, using the same C ABI as Eloquence. Every host process is then one extension's host. The built-ins are three extensions: app and other modules, eSpeak NG, and OneCore.
- Open:
  - ordering among third-party extensions, and gesture conflicts at install (M8.1);
  - the word "module";
  - the name of the "global" kind, and `applications` against `reads`;
  - the command categories;
  - native code beyond synthesizers.

## Round 5 (Dickson, 2026-11-09)

- Option 2 for native code is agreed: a Wasm module may call a native library shipped in its own extension, inside the extension's host. Full native modules stay only for roles with a small interface (synthesizer, braille driver).
- Agreed: two modules of one extension may not claim the same application. This is refused by the packaging tool, at install and at load.
- Command categories are optional. The speak-on-demand flag is left out.
- The terms "module" and "global" are kept.
- NVDA facts for the ordering decision:
  - One app module per process, found by name on the `appModules` package path.
  - Each add-on's folder is inserted at the front of that path (`addonHandler/__init__.py`, line 731), in `os.listdir` order (line 441). So the add-on whose folder comes last wins, and the others' modules for that application are silently not loaded.
  - An add-on extends the built-in module only by importing it through `nvdaBuiltin`.
  - NVDA's own synthesizers are ordinary `synthDrivers` modules on the same path, found by the same `pkgutil.iter_modules` (`synthDriverHandler.py`, line 462). Their only special case is the fallback list `["oneCore", "espeak", "silence"]` (line 501).

## Round 6 (Dickson, 2026-11-09)

- At most one installed extension may have an app module for a given application, on top of the built-in. A second is refused at install.
- The developer folder overrides as NVDA's scratchpad does (`addonHandler/packaging.py`, lines 37 to 63): its module replaces the installed one for that application only, and removing it restores the installed one. The differences from NVDA:
  - reload is automatic in developer mode;
  - the override is shown in the Extensions dialog and the log;
  - the built-in stays underneath.

  The existing reload sentence is enough. A second developer module claiming the same application fails to load and is spoken. Installs ignore the developer folder.
- Reordering is removed from the design. Within an application, the order is: the developer module, or else the installed one, then the built-in, then core behaviour.
- The extension is the unit that is enforced: pipes are made per host, and the union of its modules' `applications` and `reads` is shown at install.
- Disabling applies to a whole extension. The breaker is per module for built-ins only, since they have no disable switch, and per extension for installed ones.

## Section 2, rules (Dickson, 2026-11-09)

- The `executable` matcher is dropped. Veto rules match only properties in the listener's cache, refused at load otherwise. Global modules have no rules. Regular expressions use the `regex` crate with a size limit.
- To confirm: window rules may use `not`, with exact strings, for NVDA's Explorer exception (`appModules/explorer.py`, lines 462 to 480).
- Proposed event kinds for rules and hooks: `focus`, `name-change`, `value-change`, `states-change`, `description-change`, `suggestions-change`, `selection`, `progress`, `controlled-selection`, `notification`, `alert`, `caret`, `text-change`, `active-text-position`. These are the normalized events of `verbatim-model`'s `NormalizedEvent`, with its property changes split out. Terminal reads are not exposed. Others are added with `@since` when a module needs one.
- Agreed: one matcher grammar in `verbatim-ext-api` for window, event and node rules, hook registrations and the find reads.
  - Forms: exact, a list meaning any of, several keys meaning all of, `any`, `not`, `prefix`, `regex`.
  - Legality per subject: window and event rules take exact strings and need a positive property at the top level, so they can still be looked up by hash; node rules and hooks take every form; veto rules take only the listener's cached properties.
- Agreed: the event kind list.
- Section 2 is complete.

## Section 3, hooks (Dickson, 2026-11-09)

- `on-load` and `on-unload` are dropped, since no M5 module uses them. `adopt` is gone.
- Terminal: every `TermControl` focus whose runtime id starts 14 is dropped by a veto rule, with no hook. Recorded in the roadmap's M5 entry (working tree). `is-same-focus` is dropped.
- Hooks so far:
  - `on-focus`: pass, drop, replace or rewrite;
  - `on-event`: pass, drop or rewrite;
  - `on-node`: batched, on every emitted snapshot;
  - `status-bar`: the highest layer returning a node wins;
  - `on-command`;
  - `on-prompt-answer`.
- Event kinds for M5, cut to those with an M5 user: `selection`, `name-change`, `value-change`, `notification`, with focus going through `on-focus`. Other kinds are added by each port, with its reader change.

## Commands, list dialogs and the tray list (Dickson, 2026-11-09)

- Gestures in the manifest are a string for a single press, or `{ keys = "...", presses = N }`, with `presses` defaulting to 1. A repeat count is part of the binding. A press runs the command bound to its count, or else the highest count bound below it.
- `on-command(command-id, trigger)`, with no repeat count. `trigger` is `{ declared: option<gesture>, pressed: gesture }`, with each gesture structured as modifiers plus a main key.
  - Changing a binding keeps its declared identity.
  - A gesture the user adds to a command has `declared` none.
  - It is included in M5.
- The per-binding argument is not needed: `trigger.declared` covers it.
- The list dialog:
  - `show-list-dialog(spec) -> dialog-id`, where `spec` has a title, a label, items, and buttons with access keys and one default. Verbatim always adds Cancel.
  - The hook `on-list-dialog(dialog-id, answer)`.
  - One dialog per module, and any number across modules. A second call replaces the module's dialog in place (title, label, items, buttons; selects the first item and focuses the list) and brings it to the front.
  - Its windows appear in Alt+Tab, as NVDA's tray list does.
  - The record lives in the owning lane's record.
- The derived hold: a dialog opened by a command is held until that press's multi-press window closes, if the same keys have a command bound to a higher count. A dialog from the higher-count command replaces it. The input code stamps each press with when its window closes.
- The tray list follows `ruifontes/systrayList` (commit ff65fd6, 2026-05-10):
  - `find-window("explorer.exe", ["Shell_TrayWnd", "Windows.UI.Input.InputSite.WindowClass"], msaa)`, where later entries are descendants at any depth;
  - the tray is that window's children after the first;
  - the taskbar is the first child's children after the Start button;
  - Left Double Click is the default button.
- Windows version: `windows-version() -> {major, minor, build, revision}`, a silent fact. Matchers gain `windows-build` and `windows-revision`, with `at-least` and `below`, compiled at load.
- Clicks and prompts: no gates for now.

## Tooling and the guide (Dickson, 2026-11-09)

- WIT-first: the `.wit` files in `verbatim-ext-api` are the single source. Host bindings come from `wasmtime::component::bindgen!`, guest bindings from `wit-bindgen`, and the API reference pages from an xtask that parses the WIT with `wit-parser`, checked as current by `cargo xtask ci`. The manifest reference and a JSON Schema are generated from the `serde` types by `schemars`, with the same check. Reference pages are hand-written for review now and replaced by generated ones in M5.
- Packaging: no packaging tool. Authors zip the extension folder and rename it `.vext`, as NVDA add-ons are made, and the guide documents it. Checks of capabilities and refusals happen at install.
- The extensions guide is updated at the end of the session:
  - reference pages for the manifests, matchers, rules, hooks, events, commands and gestures, and imports;
  - updates to capabilities, limits and concepts;
  - tutorial drafts: a rules-only fix, Calculator's result, a global module with a list dialog, packaging by zip;
  - how-to outlines.

  Examples become compiled crates in the `extensions` workspace (planned), included into the book with `{{#include}}`. The design records are updated in a separate commit.

## Section 4, imports (Dickson, 2026-11-09)

- Node reading:
  - `properties()`, read from the cached snapshot;
  - parent, children, first and last child, next and previous sibling;
  - find-descendant with a depth limit, and find-ancestor, using the matcher grammar;
  - `window()`, `text()`, `focus()` and `foreground()`;
  - `find-window(application, class-path, backend)`, for app modules' own applications and global modules' `reads`;
  - errors: gone, deadline, outside, wrong-backend.
- Output:
  - `speak(message)`, where a message is application text or a Fluent id with arguments, at normal priority and queued;
  - `speak-node(node)`, announced as a focus would be, through the theme (pulled forward from M13);
  - `cancel-speech()`, effective only while the application holds attention;
  - `tone`, capped;
  - `browseable-message`, plain text.
- `pass-through()`, which is an error outside `on-command`.
- `show-list-dialog`.
- `click(node, left | left-double | right)`, shown at install as "Clicks the mouse in" the named applications.
- `windows-version()`, `clock()` and `node.process()`, which replaces the module-level process facts.
- `log`, capped.
- The settings view is dropped. No M5 module needs it, and neither does M6, since synthesizers receive their own settings through the synthesizer interface.

## Sections 5 and 6 (Dickson, 2026-11-09)

- Reserved gestures are dropped. Conflicts are surfaced instead:
  - In M5 the install dialog lists each gesture the extension shares with one already bound, by core, a built-in or another installed extension.
  - In M8.1 the same screen offers to rebind it.
  - Precedence is unchanged: the highest layer wins where it applies.
  - This replaces the reserved-gestures rule of `extensions.md` section 4.6.
- Installed extensions may ship global modules in M5. The M13 item reduces to reading every application.
- The install dialog's wording is to be reviewed separately with Dickson; he does not accept the current wording.
- Open: the limit values in section 5's table.

## Limits (Dickson, 2026-11-09)

- Focus vetoes are placed by the loader: in the listener when they name only listener-cached properties (the `ListenerEvent` shape), and otherwise in the outpost on the read node. The refusal is dropped.
- Limits:
  - the compute bound per hook: loose, then measured in step 8;
  - 10 seconds per event;
  - 64 MiB per store and 256 MiB per installed extension's host, both to measure; the built-in host's cap is measured;
  - the breaker: 3 failures in 1 minute;
  - no caps on speak, browseable messages, tones, logs, list dialogs or packages for now. This amends the "bounded output" rule.
- Added to M6.3: measure a 100,000-character utterance on OneCore and eSpeak NG, then split long utterances into pieces in the speech pipeline (NVDA issue 11843, never diagnosed by NVDA).

## The install dialog (Dickson, 2026-11-09)

- Layout, in tab order:
  - the line "Only install extensions from people you trust.";
  - read-only fields for name, version and author;
  - a read-only multi-line description;
  - an "Applications" list view (columns: Application, What it does), one row per application, merging claims, `reads` and clicks, plus an "Everywhere" row for global commands and a row for a synthesizer ("Adds the speech synthesizer <name>");
  - a "Commands" list view (columns: Command, Gesture, Where, Conflict; M8.1 adds Change gesture);
  - a "Before you install" list, shown only when it has items;
  - Install and Cancel.
- Rules versus hooks is not shown to users; it goes in developer-mode details.
- Wording:
  - native code: "Contains native code that runs outside Verbatim's extension sandbox.", where the extension sandbox is the Wasm boundary and the restricted host process is described separately as process isolation;
  - data sent out: "Can send what it reads from <application> to <host>.";
  - braille: "Connects to <braille display>.";
  - the synthesizer line has no "receives the text" clause.
- "Needs files you add yourself" is removed, along with the manifest's `user-files`. A user packages files that cannot be distributed (such as Eloquence's) into the `.vext` themselves.
- A refusal replaces the contents with one message and a Close button:
  - "Can't install X: it changes Notepad, and so does Y, which is already installed. Uninstall Y first.";
  - "Can't install X: it needs a feature Verbatim doesn't offer extensions (<interface>).";
  - "Can't install X: it contains two parts that both change Notepad."
- The Extensions dialog's details reuse the same lists.

## Write-up (2026-11-09)

- The guide was committed (ffd2d33). The design records were restated:
  - `extensions.md`, whose surface now points at the guide;
  - the M5 work plan;
  - D6 and the process list;
  - the security model's X4, X5, X6, A9 and its 2026-11-09 decision entry;
  - the roadmap's M5, M8.1 and M13 entries.
- The twin-focus design was deleted, its references now pointing at the roadmap and the experiment.
- Still open for Dickson:
  - whether eSpeak NG and OneCore become built-in extensions;
  - the developer folder's "Loaded <module>" sentence;
  - the TOML spellings of rule actions, which are provisional until the schema is generated.

## Review decisions (Dickson, 2026-11-09)

- When a global module and an app module bind the same gesture, the global module runs first, as NVDA does.
- A held dialog is withdrawn unseen, with `on-list-dialog(cancelled)` sent to its instance, when a later press runs another module's command.
- A hook that is not ready yet waits, bounded by the host's measured start-up time, then counts as `pass`.
- A read that only Core can route goes from the host to Core's reducer.
- Application identity is resolved once, by the listener, and sent to the supervisor.
- The developer folder works as one extension, as the built-ins do: module folders and a `locale/`, no manifest, one host, the breaker per module.
- eSpeak NG and OneCore become built-in extensions, each a native synthesizer module in its own host (M6). Whether the less privileged AppContainer is worth keeping is still open.
- "Loaded <module>" is accepted for a newly added module.
- The TOML spellings of rule actions are accepted.
- Every extension host runs in the ordinary AppContainer; the less privileged form is not used (Dickson, 2026-11-09).
- A native module's DLLs are listed in the manifest, and each DLL's architecture is read from its file header, never declared. Verbatim picks its own architecture, then x64, then x86, and refuses an install with none runnable.
- ARM64 installations support x86 engines through the 32-bit host, and x64 engines later through a full x64 host under emulation (estimated a day or two, not in M6).
- DLLs remain the form for native synthesizers, the built-ins included. The executable-over-pipe alternative was considered and rejected, since engine authors expect DLLs and the host handles the process work.
- wasmi was considered and rejected: no component model as of 2.0.0. Pulley within wasmtime is the interpreter option.

## Build A questions (Dickson, 2026-11-09)

- No child process starts before Core's first publication, with the time measured.
- `parent-automation-id` is accepted. `alt-held` is too specific: first check with `verbatim-inspect` whether the Alt+Tab switcher and Task View differ structurally on Windows 11. If not, consider a general `modifiers-held`. Until then the rule stays in Core.
- An MSAA item reporting both expanded and collapsed gets a third value, spoken as NVDA does, after a capture.
- The built-in failure sentence is spoken at once, unscheduled, named by its folder. Windows toasts replace it, and update notifications, in M8.6.
- Gesture order: by layer first (developer or installed, then built-in global, then built-in app, then core), and global before app within a layer.
- The Extensions dialog sits in the Verbatim menu before Preferences:
  - a list (Name, Version, Author, Status);
  - the description, and the Applications, Commands and warnings lists;
  - Install from file, Enable or Disable, Uninstall ("Uninstall <extension name>?") and Close;
  - actions take effect at once;
  - developer mode lists only the developer folder's modules, never built-ins.
- The Alt+Tab structural check is part of build A's step 2. If there is no structural difference, `modifiers-held` returns to Dickson.
- Authoring tools are on demand, not in every report:
  - build A: `verbatim-inspect matcher`, `dump-tree --properties` and `match '<matcher>'`;
  - build B: `watch-events --explain`, a module template, and `dump-tree` fixtures for the test helper.
- A developer-folder DLL is shadow-copied into the cache. A changed DLL restarts the developer folder's host.

## Build A review decisions (Dickson, 2026-11-09)

- `accept` is judged once in the listener and carried in the forwarded fact, with routing fixed to keep it and the generation.
- Verbatim's own identity is stated by Core: the supervisor records `verbatim` for Core's own process at start-up, and Core's first configuration to the listener names it. The listener records it rather than resolving it, and resolves every other process.
- The start-up failure sentence stays unscheduled until M8.6's toasts.
- Lifecycle cells:
  - a reload clears the breaker's turned-off set;
  - late effects from a `Loading` extension are dropped;
  - newer rules replace ones held in `RulesReady`;
  - a failed newer version leaves the previous version everywhere, as `extensions.md` 2.2 says.
