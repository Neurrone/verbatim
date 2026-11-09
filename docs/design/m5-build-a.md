# M5 build A: snapshot properties, identity, rules as data, the loader and the log

The design of M5's build A, settled with Dickson on 2026-11-09 and reviewed adversarially. Build A is steps 1, 2 and 3 of `docs/design/m5-extensions-work.md`. It has no extension host: every module it loads is rules only, and the host is build B. This document states what build A builds, where each part runs, the messages and states it adds, what is spoken, what it costs and how that is measured, its tests and its limits, and ends with Dickson's decisions on it and its one open point.

## 0. What this build honours, and three consequences

The decisions honoured, not reopened here:

- D6, amended 2026-11-09: every extension runs in a host process of its own, built-ins as one internal extension; no Wasm runtime in Core, the listener or an outpost; rules are data the listener and the outposts apply from tables Core publishes at a version.
- D13: the listener's callbacks make no cross-process call; it reads each UIA focus on a reading thread per application.
- D14: each event rule is defined once, judged in Core, outside Core only before a read and only from the reader's own observations; no state of Core is sent to a reader.
- Configuration versioned (Dickson, 2026-10-30): rule tables and fetch configuration carry a version published by Core alone; every judgement made under them names that version; a receiver recognises a judgement made under a version it has moved past.
- Build A's answers (Dickson, 2026-11-09), listed in "Decided by Dickson" at the end.
- 2026-11-09: the loader thread does all file and processor work and reports only finished results; the reducer owns each extension's lifecycle (`extensions.md` section 2.2) and publishes; rules publish separately from code, and the listener's `Ready` waits for the built-in extension's rules; application identity is resolved once, by the listener, from a process handle it holds, sent with the first fact for that process, and used by the supervisor; one matcher grammar in `verbatim-ext-api`; rules placed by the loader from the properties they name; the gesture map published to the hook thread at a version, a binding naming a number of presses, every press firing at once, the hook thread alone stamping `hold_until` and whether a higher count is bound; the developer folder works as one extension, watched only in developer mode; `StateSet` keeps one optional check state and one optional expansion state (Dickson, 2026-10-28).
- Build A review decisions (Dickson, 2026-11-09): `accept` is judged once in the listener and carried in the forwarded fact, and routing keeps the verdict and the generation all the way to the outpost and Core; Verbatim's own identity is stated by Core, recorded by the supervisor for Core's own process at start-up and named in Core's first configuration to the listener, which records it rather than resolving it, so Core's own outpost and the spares wait on no listener; the start-up failure sentence stays unscheduled until M8.6's toasts, and its test asserts no sentence the first focus cuts and does not change the shared deployed folder; a reload clears the breaker's turned-off set, late effects from a `Loading` extension are dropped, newer rules replace ones held in `RulesReady`, and a failed newer version leaves the previous version everywhere (`extensions.md` 2.2).

Three consequences of those decisions:

1. Holding the listener's `Ready` does not by itself make the first focus judged under the built-ins' rules, because the listener forwards MSAA facts from its start and UIA facts from the moment its registration returns, both before Core takes its `Ready` (architecture, section 1, "The focus listener"). So no child process (listener, Core's own outpost, spares) starts before Core's first publication, and the first publication waits until the built-in extension has left `Loading`; each child takes its configuration as its first command (Dickson, 2026-11-09). The listener's `Ready` therefore follows the built-in rules by construction. Core's own outpost and the spares wait for that publication but not for the listener, since Core states its own identity (section 4.2). The time this adds before the children start, and before Core's own outpost and the first spare are ready, is measured and recorded (section 15).
2. The rule surface gains the node matcher property `parent-automation-id` and the node action `containers = "none"`, which drops the focus's containers (Dickson, 2026-11-09). Task Manager's node rule uses the property (`appModules/taskmgr.py` 14 to 22). The Alt+Tab switcher items' dropped containers (`crate::applications::drops_ancestry`, NVDA's `appModules/explorer.py` 39 to 45 and 303 to 311) use the action, matched by whatever structure tells the switcher from Task View, which step 2 establishes (section 5.5); if no structure does, the rule stays in Core, keyed by the outpost's identity, and the open point at the end goes to Dickson.
3. With no host, module commands have nothing to run them in build A, so the published gesture map carries Core's bindings and the module part of the map is empty in production; the press-count resolution and `hold_until` are built and tested on the pure decision machine. The loader refuses `[[commands]]`, `[[hooks]]`, `component` and `kind = "global"` in build A with a stated reason, lifted in build B.

## 1. Terms

Defined here once; the glossary entries are written in step 10.

- **Identity**: the name an application's modules claim it by, resolved from its process (section 4).
- **Generation**: one publication by Core's reducer of every reader's configuration: the rule tables of every active extension, the fetch configuration and the gesture map. A positive integer, minted only by the reducer, increasing by one per publication.
- **Slice**: the part of a generation one reader is sent: the listener's (every identity's listener-placed rules), an outpost's (its identity's rules and the fetches), the hook thread's (the gesture map).
- **Layer**: where a module sits for one application, highest first: developer folder, installed extension, built-in, then Verbatim's own behaviour.
- **Placement**: where the loader puts a rule to run: the listener, the outpost, or Core.
- **Rule reference**: an extension, module and rule id, printed `<module>/<rule id>`, which every judgement made by a rule names together with its generation.

## 2. The parts and where each runs

| Part | Process and thread | Crate |
|---|---|---|
| Snapshot properties | each outpost, on its worker as it builds snapshots | `verbatim-model`, `verbatim-uia`, `verbatim-ia2`, `verbatim-outpost` |
| `StateSet`'s check and expansion | the readers that map states; Core where it speaks them | `verbatim-model`, `verbatim-uia`, `verbatim-ia2`, `verbatim-core` |
| Identity resolution | the listener: its outgoing backlog, under the transport's existing lock, when the first message naming the process is sent, whichever thread sends it (section 4.2); Verbatim's own process is stated by Core, never resolved | `verbatim-listener`, `verbatim-listener-logic` (the resolver, a pure function over the kernel calls' answers), `verbatim-process` (`stem_of`) |
| Identity's use | the supervisor's owner thread; Core's reducer; the hook thread through Core's view | `verbatim-supervisor`, `verbatim-core`, `verbatim-input-windows` |
| The rule data: matchers, properties, rules, rule references, generations, slices | carried in Core's inputs and effects, the flight recorder and the pipes | `verbatim-model` (platform neutral; `Clone`, `Eq`, `Serialize`, a regular expression held as its source text; section 8.3) |
| Manifests, parsing, validation, compilation of regular expressions, placement, evaluation | wherever called: the loader parses and places, each reader compiles its slice's regular expressions at its boundary, readers and Core evaluate | `verbatim-ext-api` (platform neutral, original code; depends on `verbatim-model`) |
| NVDA's class verdict and normalization composed with rules | the listener's reading threads and each outpost's arbitrator | `verbatim-reader-rules` |
| Built-in module folders | `extensions/builtin/<module>/module.toml` in the repository, placed under `target/debug/extensions/builtin/` by `cargo xtask build-extensions` | data, GPL like the code it was ported from |
| The loader | Core, the loader thread | `verbatim-app` (thread and file work), `verbatim-ext-api` (parse, validate, compile) |
| The extension lifecycle, the generation, publication | Core, the reducer thread | `verbatim-core` |
| Sending each slice | Core: the supervisor's owner thread for each child's first command and for every configuration of the listener; the reducer thread through the supervisor's writers for an outpost's later ones | `verbatim-app`, `verbatim-supervisor` |
| Applying slices | the listener's I/O thread compiles and its reading threads apply; each outpost's I/O thread compiles, its reducer holds and its worker applies; Core's reducer | as above |
| The gesture map at a version | Core's reducer publishes; the hook thread reads | `verbatim-input`, `verbatim-input-windows` |
| The log ring | every Core thread that logs, except the hook and audio threads, which never log; a monitor with one mutex (section 11.1) | `verbatim-app` |
| The log viewer | Core, the GUI thread | `verbatim-gui` |
| Verbatim+F1 | the hook raises it; the reducer asks the navigator's outpost and writes the report through an effect; the GUI shows it | `verbatim-input`, `verbatim-core`, `verbatim-outpost`, `verbatim-gui` |

The tiers hold: `verbatim-ext-api` is platform neutral and depends on no GPL-tier crate, only on `verbatim-model`, whose types its evaluator reads, so `verbatim-model` names no `verbatim-ext-api` type and no cycle forms; `verbatim-core` depends on `verbatim-ext-api` for the published types and the evaluator; `verbatim-reader-rules` (GPL logic) depends on `verbatim-ext-api` to compose NVDA's ported class lists with the tables. Every crate on a reducer's path keeps the testing standard's rule of no hash map or set ("The crates on a reducer's path"), so every table is a `BTreeMap`.

## 3. Step 1: snapshot properties and the check and expansion states

### 3.1 The properties

`NodeDetails` gains, each `Option`, left out of the message when `None`:

| Field | Filled for | Read from | Calls added |
|---|---|---|---|
| `uia_class_name` | every UIA snapshot (today's `class_name`, renamed, now on every UIA snapshot, not only a focus's and its ancestors') | the element's cache | none: added to the cache requests that lack it |
| `automation_id` | every UIA snapshot | cache | none |
| `framework` | every UIA snapshot | cache | none |
| `runtime_id` | every UIA snapshot | `GetRuntimeId`, a local read on a cached element | none |
| `control_type` | every UIA snapshot, the raw id the role is mapped from | cache (`RoleCore`) | none |
| `window_class` | every snapshot with a window | the outpost's window cache, normalized as rules see it (section 6.4) | none on a cache hit; one local `GetClassNameW` on a miss |
| `msaa_role` | every MSAA snapshot, the raw `accRole` number | the read already made | none |
| `control_id` | every MSAA snapshot whose window is a child window | `GetDlgCtrlID` of the snapshot's window | one local call per MSAA snapshot, which sends no message to the application |
| `parent_automation_id` | every UIA focus, as today (Core's switcher rule reads it for File Explorer, whose rules do not name the property); any other UIA snapshot only when the identity's rules name `parent-automation-id`, and only once every other property of such a rule's matcher has matched | the focus's ancestor chain, already read; for a snapshot with no chain, its parent's cached automation id | none for a focus; one parent read for another snapshot whose other matcher properties matched, in its remote operation where remote operations are on |
| `presentation` | set only by a node rule's `presentation` action | the rule | none |

`NodeSnapshot` gains `provenance`, a list, empty unless a rule changed the node: each entry the field changed, the rule reference, the generation, the value it had before and, when the rule overrode another rule of its module on that field, the overridden rule's reference (section 5.4), so the focus report and Verbatim+F1 say "role: edit, set by notepad/goto-line-is-edit, was dialog from UIA" (the rules reference page's wording).

The outpost always fills these on the snapshot it judges, since its rules read them; carrying them to Core costs bytes on every snapshot, which step 1's ledger entry records; nothing is designed around that copy unless the measurement shows it matters (CLAUDE.md, "Judge every change"). The listener's `UiaSnapshotFact` gains `runtime_id`, `automation_id`, `framework` and `uia_class_name` from the event's cache, which it already holds.

`presentation` is read by the outpost's one filter of focus ancestors (`verbatim_uia::client::take_reported` and `is_presentable_focus_ancestor`): `Some(Content)` makes an ancestor presentable whatever the layout judgement says, `Some(Layout)` makes it not presentable. Verbatim's object navigation sees the whole tree (NVDA with simple review off, the recorded decision on `Uia::neighbor`), so presentation changes only which ancestors are spoken. Task Manager's node rule (section 7.3) therefore changes nothing anyone hears in build A: it matches the cells inside a row, which are never ancestors of a focus on the row, and NVDA's rule matters under simple review, which Verbatim does not have. It is carried as data, its provenance proven against `mockapp`, and its parent read costs nothing in practice until Task Manager can be read (M8.7), since Task Manager is the only identity whose rules name the property.

### 3.2 One optional check state and one optional expansion state

`StateSet` loses `Checked`, `Mixed`, `Expanded` and `Collapsed` as bits and gains two fields, so a node that is both checked and half checked cannot be written (Dickson, 2026-10-28; CLAUDE.md, "Make invalid states unrepresentable"):

- `check: Option<Check>`, `Check` being `Checked` or `Mixed`. `None` is not checked; whether a node can be checked stays the `Checkable` state and its role, as now.
- `expansion: Option<Expansion>`, `Expansion` being `Expanded`, `Collapsed` or `ExpandedAndCollapsed`; the third is written only by the MSAA reader (Dickson, 2026-11-09).

The readers parse once, at the boundary:

- UIA: the Toggle pattern's one state maps to one value (`Indeterminate` to `Mixed`, `On` to `Checked`), and the ExpandCollapse pattern's one state to one value, so UIA cannot report both (NVDA, `NVDAObjects/UIA/__init__.py` 2065 to 2085).
- MSAA: `STATE_SYSTEM_MIXED` with `STATE_SYSTEM_CHECKED` maps to `Mixed`, as NVDA discards `CHECKED` when `HALFCHECKED` is present (`NVDAObjects/IAccessible/__init__.py` 1141 and 1142). Today Verbatim says "checked half checked" for such an object, NVDA "half checked"; the change matches NVDA, so it needs no decision, and a capture confirms it before it is built, as the work plan says.
- MSAA `STATE_SYSTEM_EXPANDED` with `STATE_SYSTEM_COLLAPSED`: NVDA keeps both and speaks both, in its state order ("expanded collapsed": `controlTypes/state.py` 56 and 57, sorted in `controlTypes/processAndLabelStates.py` 203), and Verbatim does too today. Both bits map to `ExpandedAndCollapsed`, spoken "expanded collapsed" as NVDA speaks it, confirmed against a capture before it is built (Dickson, 2026-11-09).

Core's state speech (`verbatim-core`'s `spoken_states`, `negated_states`, `STATE_ORDER`) reads the two fields where it read the four bits, with the same order; `ExpandedAndCollapsed` is spoken as the two words in that order, so what is said changes only for the half-checked case above. A state change's diff maps each field's value to the words it is spoken as (`ExpandedAndCollapsed` to "expanded" and "collapsed", `Mixed` to "half checked") and speaks the words gained and lost, as today's set diff does (`reduce.rs`, `state_change_segments`): `Expanded` to `ExpandedAndCollapsed` speaks only the gained "collapsed", and `Mixed` to `Checked` speaks as today's rule for a lost `Mixed` with `Checked` present.

## 4. Step 1: application identity, one resolver

### 4.1 The resolver

The listener resolves a process's identity once, from a process handle it holds, by NVDA's rules (`appModuleHandler.py` 145 to 175):

| Process | Identity |
|---|---|
| Any | its executable's file name without the extension, lower-cased, dots kept (`verbatim_process::held::stem_of`) |
| A console host, the process a console window's facts are routed to | the console client's identity: the stem of the client process, which a fact names as the process `GetWindowThreadProcessId` gives for its window (the listener's `routed_process` already reads it), and an `Identify` names as its `client` (section 4.2) |
| `wwahost.exe` | the part after `!` of its application user model id (`GetApplicationUserModelId`), lower-cased; its own stem when that is empty (`appModules/wwahost.py` 18 to 38) |
| `msedgewebview2.exe` | its parent's identity (`NtQueryInformationProcess`, `ProcessBasicInformation`), only when the parent process is still the one that created it, judged by its creation time being earlier than the child's; its own stem otherwise, as NVDA falls back on `LookupError` (`appModules/msedgewebview2.py` 12 and after; `appModuleHandler.py` 172 to 175) |
| Verbatim's own process | `verbatim`, stated by Core in the listener's configuration and never resolved (Dickson, 2026-11-09) |
| A process that cannot be opened | `Unknown`, which no module claims, as NVDA's empty name finds no module |

`AppIdentity` is parsed once: a non-empty lower-case name, or `Unknown`. Every call in the table is a kernel call that waits on no application (`OpenProcess` with limited query access, `QueryFullProcessImageNameW`, `GetProcessTimes`, `GetApplicationUserModelId`, `NtQueryInformationProcess`, `GetWindowThreadProcessId`), the same class the listener already makes for integrity levels, so D13 holds. The resolver is a pure function in `verbatim-listener-logic` over those calls' answers (the image path, the user model id, the parent's id and both creation times, the client's image path), so each row is unit tested on any host; only the thin layer that makes the calls is Windows code.

### 4.2 Where it is kept, and its one ending

The listener keeps one record per process id that it could open: the handle, the creation time and the identity. While the handle is open, Windows does not reuse the id, so the record can never name another process. Its one owner is the listener's outgoing backlog (`Waiting` in `listener.rs`), the value every message to Core passes through under the transport's existing lock, beside the `hooked` map it already keeps per process; no lock is added. The first message naming a process that has no record, a UIA callback's marker, an MSAA fact forwarded from the `WinEvent` callback, a reading thread's fact, a `Foreground` report or an I/O thread's `Identify` answer, creates the record in the backlog's `push`, under that lock: it makes the kernel calls, inserts the record, and pushes `ListenerToCore::Process { pid, identity }` as an urgent message ahead of the message that created it. Because the check, the creation and both pushes happen in one hold of the one lock every sender takes, no fact for a process can be sent before its `Process`, whichever callback, reading or I/O thread sends it, and two threads cannot both create a record: the second finds the first's. The kernel calls are made under the lock once per process, a hold of tens of microseconds, measured (section 15). A `Foreground` report takes the window's process identity from the same record in the same hold. A reading thread judging a UIA event reads its application's identity from the record with one more brief hold (`Sender::inspect`, a read accessor added to the transport), the record already made by the marker the callback sent before queuing the element.

A process that cannot be opened (it has exited, or refuses even limited query access) gets no record: `Process { pid, Unknown }` is pushed ahead of each of its messages instead, so a late event from an exited process cannot leave a record without a handle for a later process with the same id to inherit.

The record ends only at `CoreToListener::ApplicationExited`, which the listener already answers with `ExitHandled`, and with the listener. To give every record that ending, the supervisor holds the process from `Process`, as it holds one from its first fact today (`owner.rs`, `hold_target`), and so sends `ApplicationExited` at its exit; when the hold fails because the process has already exited, the supervisor sends `ApplicationExited` at once. A `Process` for a pid the supervisor already holds (a replaced listener, which starts with no records, resolving it again) is ignored: the supervisor's held handle keeps the id the same process, so the identity is the same.

Verbatim's own process is the one exception to resolution (Dickson, 2026-11-09): at start-up the supervisor records the identity `verbatim` for Core's own process id itself, so Core's own outpost and the spares start as today, with no wait on the listener. Core's first configuration to the listener names that process id and identity (`CoreToListener::Configure`'s `own`), and the listener records it, opening its handle as for any record, rather than resolving it, and sends no `Process` for it. The listener resolves every other process. One owner per process's identity: Core for its own, the listener for every other.

The supervisor keeps the identity in its application record and puts it in `CoreToOutpost::Assign`, in `OutpostMessage::Started` for the app, and in the outpost's log file's name in place of its own `image_stem`, which is deleted. An outpost started with no fact before it for a process other than Core's own (`Supervisor::ensure_spawned`: the taskbar's process for the tray list, a focus wanted for a pid) first sends the listener `CoreToListener::Identify { pid, client }`, where `client` is the console client the app knows when the pid is a console host (`want_focus`'s `(pid, server)` pair), so a console host's identity is its client's whichever message creates its record; the application's record waits in a new state, `Identifying`, until the listener's `Process` answer (section 13, table 4). The listener answers every `Identify` with one `Process`, from the record when there is one. When the listener is replaced, every application still `Identifying` is asked again of the new listener once it starts.

`ListenerToCore::Foreground` gains the window's process identity, so Core's attention record holds the identity of the application in front from the one report that moves it; `HookForeground` gains it too, so the hook thread knows which application's commands apply (section 10). Core's reducer keeps each outpost's identity in its record of that outpost, from `Started`, beside the attention record. The outpost's own application name (`WindowFacts::application`, read per focus by `Outpost::application_name`) is deleted: it was a second resolver.

## 5. Step 2: manifests, the matcher grammar and rules in `verbatim-ext-api`

### 5.1 The manifest types

`verbatim-ext-api` holds `ExtensionManifest` and `ModuleManifest` as the guide's manifests page lists them, read with `serde` and `deny_unknown_fields`, then parsed into validated types whose constructors are private, so code past the loader never checks a manifest again. The WIT files are written in this crate in build A for Dickson's approval, interface by interface, as step 2 says, and nothing compiles against them until build B.

Build A loads only internal extensions (the built-in extension), and refuses, with these exact messages, what needs the host:

- a `component`: "<module>: module code needs the extension host, which this build of Verbatim does not have";
- `[[hooks]]`: "<module>: hooks need the extension host, which this build of Verbatim does not have";
- `[[commands]]`: "<module>: commands need the extension host, which this build of Verbatim does not have";
- `kind = "global"` or `"synthesizer"`: "<module>: <kind> modules need the extension host, which this build of Verbatim does not have";
- `sleep-mode`: "<module>: sleep mode is not built yet".

### 5.2 The matcher grammar

One type, `Matcher`, parsed from a TOML value into a tree that can only hold the forms the matchers page lists: an exact value, a list meaning any of, several keys meaning all of, `all`, `any`, `not`, `prefix`, `regex` (the `regex` crate with its size limit, Dickson, 2026-11-09), `at-least` and `below`. Properties are one enum, `Property`, with the backend each belongs to and the type of its value; a value of the wrong type is a parse error, so a matcher never compares a number with text. Beside the matchers page's properties, `Property` holds `parent-automation-id` (UIA nodes: the automation id of the node's parent in the raw tree; Dickson, 2026-11-09), which the matchers page gains.

Evaluation is a function over a `Properties` trait that each evaluator implements over what it holds: the listener over the event's cache, the window's classes and the identity; the outpost over its snapshot, details and window facts; Core over a focus report's window facts. A property the subject does not have (an MSAA node's automation id, a UIA node with no automation id) is absent: every positive form fails on an absent property and `not` of it succeeds, as NVDA compares `None` with a string and finds them unequal.

`windows-build` and `windows-revision` are folded at load: the loader reads the version once (`RtlGetVersion`) and replaces each such comparison by its truth, simplifying `all`, `any` and `not` around it; a rule whose matcher folds to false is dropped from the tables, and one whose matcher folds to true at its top level keeps its other properties. So no reader ever evaluates the version.

Legality per use is checked at load:

- window and event rules: exact text and lists, `all`, `any`, `not`, `at-least`, `below`, with at least one positive property named at the top level, which becomes the key the rule is filed under;
- node rules, hook registrations (build B) and the find reads (build B): every form, every control property of the backend the rule names.

### 5.3 The rules

`WindowRule`, `EventRule` and `NodeRule`, each with its id, its matcher and its typed actions as the rules page lists them. One addition decided here: a window rule's `class` and `root-class`, like `window-class`, are the classes after NVDA's built-in normalization (`verbatim_reader_rules::class::normalize_class_name`), because NVDA's app modules judge the normalized class, Task Manager's included (`appModules/taskmgr.py` 39 and 40). A `normalize-class` rule matches that class and is applied once, before every other window rule and every node rule, so everything after sees its result. Node rules gain the action `containers = "none"`, applied to a focus only: the focus is reported with no containers, so none is spoken (Dickson, 2026-11-09); the rules page gains it.

### 5.4 Refusals

A module is refused at load with its reason, each pinned by one unit test: an unknown key, property, action or event kind; a property of the other backend than the rule names; a window or event rule with no positive property at its top level; a regular expression past the size limit; a missing or duplicate rule id; a rule in a global module; a module whose `applications` is empty; two modules of the built-in extension claiming one identity (the later in ordinal order of folder names fails, decided from the whole scan, never from arrival order); and the build A refusals of section 5.1.

There is no conflict check between rules (Dickson, 2026-11-09): within one module, rules apply in manifest order and the later wins any property both set, for every subject; nothing is refused or warned of for an overlap, and the provenance entry of a node where two rules set one property names the rule that won and the rule it overrode, so Verbatim+F1 shows both.


### 5.5 The switcher check

Step 2 dumps, on Windows 11 with `verbatim-inspect dump-tree --properties` (section 11.4), the Alt+Tab switcher and Task View, each with a window focused, and compares the window class, the root class, the list's automation id and the item's framework (Dickson, 2026-11-09). If they differ in one of these, the Explorer module's switcher rule is a node rule matching that difference with `containers = "none"`, and `drops_ancestry` is deleted from Core. If they do not, a general `modifiers-held` property goes back to Dickson (the open point at the end), and the rule stays in Core, keyed by the outpost's identity `explorer` rather than `WindowFacts::application`, until he decides. The dumps are kept in the scratchpad; the rule cites the property it matches.

## 6. Step 2: compilation, placement and evaluation

### 6.1 Compiled tables

The loader parses each module into `ModuleRules`, a `verbatim-model` value (section 8.3), immutable and shared through `Arc` inside Core:

- window rules filed by their key property and value (`class`, `root-class`), with the full matcher kept for the candidates;
- event rules filed by `kind`;
- node rules in the order written, split by backend;
- each rule's placement (section 6.2) and its rule reference.

The reducer assembles each identity's rules as a list of `Arc<ModuleRules>`, one per layer that claims the identity, lowest layer first, so publishing a generation copies no table and merges nothing. Readers apply each layer's rules in that order, so the highest layer's action wins a contested property, and a drop from any layer stands (`extensions.md` section 6). A lookup costs one ordered-map probe per layer, at most three.

### 6.2 Placement

A pure function of the properties a rule names (`extensions.md` section 4, Dickson, 2026-11-09):

| Rule | Placed in |
|---|---|
| Focus veto (`veto-focus`) naming only properties the listener's event cache holds | the listener, before its read; and the outpost, on every focus the outpost reads itself (a focus-now answer, a replacement outpost's current focus, the outpost's own read of a focus UI Automation did not deliver, a live re-read) |
| Any other focus veto | the outpost, on every focus it reports, the listener's reads included |
| A drop, an event rule's or a node rule's `drop` | the first reader that receives the event and has every property it names: the listener for UIA selections, notifications and menu openings when it does; the outpost otherwise; Core when no reader has them (no M5 property is outside both readers, so no rule lands there in build A) |
| An event rule's `accept` | the listener, which receives every UIA notification first, judged once and carried in the forwarded fact (Dickson, 2026-11-09; section 6.5) |
| Window `normalize-class` | every reader that evaluates a class: the listener, the outposts and Core |
| Window `backend` | the outposts' arbitrators, and the listener's probe-free skip |
| Window `silent` | Core |
| Node rules other than vetoes and drops, and redirects | the outpost's worker, as it builds each snapshot |

The properties the listener's cache holds are those of `verbatim_uia::shape::ListenerEvent`: `role` (computed from `RoleCore` with the same function the outpost uses), `control-type`, `uia-class-name`, `automation-id`, `framework`, `name`, `runtime-id`, and `window-class`, `class` and `root-class` from the classes the listener reads for its checks. A unit test in `verbatim-listener` pins that each property the placement function calls listener-held is read from that shape. `role` is added to the guide's list ("Where rules run and what they cost"), which named the others.

A veto never applies to a foreground change's focus (a whole-window focus), as Core's refusal does not today, and NVDA's handler refuses only focus and menu-opening events (`UIAHandler/__init__.py`, as the parity ledger cites it).

### 6.3 Evaluation in each reader

- **The listener.** Each reading thread loads the newest listener slice (section 9.2) at the start of judging each event, its boundary, and reads the identity of the event's application from its record (section 4.2); for the class verdict, that is the identity of the window's process, as NVDA consults the window's process's module (`UIAHandler/__init__.py` 1225 to 1231), which is the routed process's identity in every case, a console host's record naming its client. For a UIA focus or menu opening, the listener-placed vetoes run as a new step of `verbatim_listener_logic::checks::next`, after the hidden-frame refusal and before the keyboard focus read, where NVDA's object refuses its own focus (`shouldAllowUIAFocusEvent`); a veto drops the focus with the reason "vetoed by <module>/<rule id>, generation <n>", following its marker as every drop does. The class verdict step consults the identity's window `backend` rules (section 6.4). For a selection or notification, the listener-placed drops run in `FocusReader::check_fact`, and a matching `accept` is carried on the fact (section 6.5). Each fact the listener sends carries the generation it was judged under.
- **The outpost.** The reducer holds its slice and applies a new one between two inputs; each program it dispatches carries an `Arc` of the slice it was dispatched under, and its results name that generation. The worker applies node rules, in layer order then written order, to each snapshot it builds, records provenance, applies outpost-placed vetoes to focuses (and listener-placed ones to the focuses it reads itself), outpost-placed drops to events, and redirects. A redirect to the parent uses the chain the focus carries, with no call; to the first child, one call; to a descendant by automation id, one `FindFirst` on the descendants, in a remote operation where remote operations are on, after the outpost finds its own element by the chain's runtime ids. A vetoed focus is not reported; a vetoed focus-now answer is answered `FocusNowOutcome::Vetoed { rule, generation }`, on which Core does nothing, as its refusal does today; a vetoed live re-read is answered `OutpostToCore::LiveRereadGone`, the answer it already gets when the outpost drops the read that would answer it, so Core's request has its one answer. A new generation drops the arbitrator's kept verdicts, since a window `backend` rule may have changed (roadmap M5, "kept verdicts dropped when rule tables are reloaded"). A listener focus fact names the generation it was judged under (section 6.5). When that generation differs from the outpost's, the outpost judges the fact's window by its own verdict: a focus the listener dropped by its class skip (`UiaFocusDropped` with that reason) whose window the outpost judges UIA is read by the outpost itself, as it reads a focus UI Automation did not deliver; a focus the listener read whose window the outpost judges MSAA is dropped, the outpost's own hooked MSAA focus standing. So under two generations a focus is spoken once, by the outpost's verdict, never lost and never twice. The outpost copies the listener's generation and a notification's carried `accept` into the event it reports (section 6.5).
- **Core.** The reducer judges `silent` on each focus report against the outpost's identity's rules under its current generation, which is never stale since Core holds the newest; and takes `accept` from the verdict the reported notification carries (`NormalizedEvent::Notification`'s `accepted`), judging nothing again.

### 6.4 The class verdict with rules

`verbatim_reader_rules::class::class_verdict` takes the identity's window rules as a second argument and keeps NVDA's order (`UIAHandler/__init__.py` 1220 to 1244): NVDA's good list first; then the module rules' `backend`, the highest layer's matching rule deciding; then NVDA's bad list; then the probe. NVDA checks a module's good verdict before the bad list and its bad verdict after it, which gives the same answers as one module step before the bad list, since the bad list itself says MSAA. `SHELL_ROOT_CLASSES` and `WindowClasses::is_shell` leave `verbatim-reader-rules` for the Explorer built-in (section 7.3). The listener's skip by class (`MSAA_CLASS`) and every outpost's arbitrator call this one function, so under one generation they agree (ADR 0001, "One judge per fact"); under two generations, the outpost judges the listener's skip by the generation the dropped fact names (section 6.3).

### 6.5 `accept` judged once, in the listener, and carried

NVDA judges File Explorer's snap results before the read, in the UIA handler (`UIAHandler/__init__.py` 1089 to 1100, calling `appModules/explorer.py` 519 to 535), and three places in Verbatim need that one verdict today: the listener's drop of a notification with no window (`focus_read.rs`, `NOTIFICATION_NO_WINDOW`), the supervisor's choice to start an outpost for a fact (`verbatim_core::supervisor::policy::may_start_outpost`) and Core's acceptance (`reduce.rs`, the notification arm). `accept` is judged once, in the listener, and carried in the forwarded fact (Dickson, 2026-11-09, changing `extensions.md` section 4's placement in Core's reducer). The verdict travels inside the fact, never beside it, so routing cannot drop it:

- `DeliveredFact::UiaNotification` gains `accepted: Option<RuleReference>`, set by `FocusReader::check_fact`, which also passes a notification with no window when it is accepted, in place of today's `SNAP_RESULTS_ACTIVITY` comparison;
- `ListenerFact` gains the listener's `generation`, and the supervisor copies it into `CoreToOutpost::DeliverFact`, which gains `generation`, so the fact the outpost receives names the generation the listener judged it under;
- `may_start_outpost` reads `accepted.is_some()` for a notification;
- the outpost copies both into `NormalizedEvent::Notification`, which gains `accepted: Option<(RuleReference, Generation)>`, and into the event's report;
- Core's notification arm takes `Acceptance::Background` when `accepted` is present, and `SNAP_RESULTS_ACTIVITY` is deleted from Core.

### 6.6 What a stale judgement does

A judgement names its generation; Core, holding the newest, acts by the judgement's kind (`extensions.md` section 2.2):

| Judgement | Made in | Core's action when its generation is older than Core's |
|---|---|---|
| A focus veto or a drop | listener or outpost | stands: the event never reaches Core, or reaches it only as a vetoed focus-now answer, and a dropped event cannot be read again |
| An `accept` | listener, carried to Core in the notification (section 6.5) | used |
| The class skip of a UIA focus (`MSAA_CLASS`), or its absence | listener | none in Core: the outpost, holding its own generation, re-judges a listener focus fact whose generation differs from its own by its own verdict (section 6.3), so the focus is spoken once whichever reader moved first |
| Node rule changes and their provenance | outpost | used |
| A backend verdict | outpost | used; the outpost drops its kept verdicts at the new generation |
| `silent` | Core | never stale |
| A module command (build B) | hook thread | dropped when the module's extension is not `Active` or the module is turned off, as for any effect |
| A hook's effect (build B) | host, through the outpost or Core | dropped when the module's extension is `Disabled`, `Failed` or `Loading`, or the module is turned off; otherwise used |

Build A has no effects and no commands, so in build A every stale judgement stands or is used, and the version is logged and shown by Verbatim+F1; the action table is built and tested now so that build B adds producers, not rules.

## 7. Step 3: the built-in module folders and the loader

### 7.1 The folders

`extensions/builtin/<module>/module.toml` in the repository, with no extension manifest; built-in strings are in Verbatim's own Fluent resources. `cargo xtask build-extensions` copies them to `target/debug/extensions/builtin/`, which the end-to-end harness's deploy and the VM image carry beside the executables. The built-ins are found beside the executable, never under the configuration root (roadmap M5, "Built for the installed copy").

### 7.2 The loader thread

A thread of Core's, started by the main thread just after it posts `Input::Started`, so the start sound always comes before any failure sentence. It is given the built-in folder's path as an argument (production passes the folder beside the executable; a test passes a folder of its own), so no environment variable changes where it reads. It owns every file read and all parsing, validation and compilation, and reports finished results only (Dickson, 2026-11-09). In build A it does, at start:

1. Read the version of Windows once.
2. List the built-in folder; read every `module.toml`; parse, validate (every regular expression compiled once to check it, then kept as its source text, section 8.3) and place each module independently; decide conflicts from the whole set found.
3. Send the reducer one `Input::ExtensionRules { extension: Builtin, version, result }`, where a successful result holds the parsed modules, as `verbatim-model` data, and the modules that failed, each with its reason, and a failed result holds a reason (the folder unreadable or empty).
4. Send `Input::ExtensionCode { extension: Builtin, version, result }`, which in build A is always "no components", since build A refuses components.

The loader mints each extension's versions, increasing, so the reducer recognises a late result of an older version. Each load has one answer path (CLAUDE.md, "Every request and waiter has exactly one answer path"): the loader holds, for each load it works on, a token for the result not yet sent, and the token's drop, when the thread unwinds from a panic in parsing or compiling, sends `ExtensionRules { result: failed("the loader stopped: <panic message>") }` for that version, so the start gate opens and the failure is spoken (section 14). A panic unwinds only the loader thread, since the workspace does not abort on panic and the panic hook chains and returns (`flight_dump::install_panic_hook`). Once the loader thread has ended, the shell answers each later `Effect::Load` at once with a failed rules result, its channel being closed. A read that hangs rather than fails (a portable copy on a network share that stops answering) holds the start until it returns; the guardian's start-up progress bounds it once M4.8 lands, and until then it is a stated limitation (section 17). The thread then waits on its channel for the reducer's `Effect::Load` requests, which build A's reducer never makes after start (they come from enable and reload in build B), and exits at shutdown. The built-in extension's unit of failure is the module: a module that fails is left out and reported, and the rest load (work plan, step 3 tests). The same holds for the developer-folder extension in build B; an installed extension fails whole.

### 7.3 The built-ins as data

The rules that leave Core and `verbatim-reader-rules`:

| Today | Becomes, in `extensions/builtin/explorer/module.toml` (planned) or `taskmgr/` | NVDA |
|---|---|---|
| `SHELL_ROOT_CLASSES` and `is_shell` (`verbatim-reader-rules`) | window rule `backend = "uia"`, `match = { root-class = [the four classes], not = { class = "Start" }, windows-build = { at-least = 22000 } }` | `explorer.py` 462 to 480 |
| `SHELL_STAGING_CLASSES` (`verbatim-core`) | window rule `silent = ["focus", "foreground"]`, `class = ["ForegroundStaging", "LauncherTipWnd", "ApplicationManager_DesktopShellWindow"]` | Verbatim's parity entry, "The shell's staging windows" |
| `refuses_focus` (`verbatim-core`) | node rule, `backend = "uia"`, `veto-focus = true`, `match = { any = [{ uia-class-name = "MultitaskingViewFrame", role = "window" }, { uia-class-name = "Windows.UI.Input.InputSite.WindowClass", role = "pane" }] }`, placed in the listener and the outpost | `explorer.py` 34 to 36 and 290 to 302 |
| `SNAP_RESULTS_ACTIVITY` (`verbatim-core`, three users) | event rule `accept`, `match = { kind = "notification", activity-id = "Windows.Shell.SnapComponent.SnapHotKeyResults" }` | `explorer.py` 519 to 535 and 548 to 550 |
| `drops_ancestry` (`verbatim-core`) | node rule `containers = "none"`, matching the structure that tells the switcher from Task View (section 5.5); or, when none does, stays in Core keyed by the identity `explorer` | `explorer.py` 39 to 45 and 303 to 311 |
| Task Manager, not in Verbatim today | window rule `backend = "msaa"`, `class = "SysListView32"`; node rule `presentation = "content"`, `backend = "uia"`, `match = { parent-automation-id = "TmViewRow", not = { automation-id = "TmRowIcon" } }` | `taskmgr.py` 14 to 49 |

The staging rule becomes scoped to File Explorer's process, where today it applies to those classes in any process; all three classes are the shell's own. The shell-root rule likewise becomes scoped to File Explorer's windows, where `is_shell` applies today in any process; that matches NVDA, which consults the window's process's module (`UIAHandler/__init__.py` 1225 to 1231). Step 2's dumps record which process owns each window under the four roots; if any is not File Explorer's, the change is reported to Dickson as a difference in what is read rather than called unchanged, and the rule is filed in that process's built-in module as well, as NVDA would need a module there.

The guardian's last restart passes `--extensions-off` (M4.8, `docs/design/guardian.md`). The built-in extension still loads then: its rules are Verbatim's own behaviour moved out of Core's code, the start gate waits on it, and it runs no module code; `--extensions-off` turns off installed extensions and the developer folder.

## 8. Step 3: the reducer's extension lifecycle and publication

### 8.1 The record

The reducer keeps one record per extension: its newest version seen, and its lifecycle, one value of:

- `Loading`;
- `RulesReady { version, rules }`;
- `Active { version, rules, code, turned_off, pending }`, where `turned_off` is the set of built-in or developer-folder modules the breaker turned off (empty in build A, which has no breaker), and `pending` is `Idle` or `AwaitingCode { version, rules }`, a newer version whose rules arrived first;
- `Failed { reason }`;
- `Disabled { by }`, by the user or the breaker.

A result whose version is not newer than the newest seen, or a code result whose version is not the one whose rules the state holds, is stale and dropped, so no state needs a flag for it. A result dropped in `Disabled` still updates the newest version seen, so a version in flight when the extension was disabled is never taken later. A reload is the arrival of a newer version's rules result, which is how build B's developer-mode reload will reach the reducer; build A's loader never sends one after start, but the transitions are built and tested now.

### 8.2 The table

Each cell says what the rule does in that state. "Publish" means: mint the next generation and emit `Effect::Publish` with every reader's slice (section 9), unless the start gate (row 4) holds it.

| Rule | `Loading` | `RulesReady` | `Active`, `Idle` | `Active`, `AwaitingCode` | `Failed` | `Disabled` |
|---|---|---|---|---|---|---|
| 1. Rules result, newer version, successful | to `RulesReady`; publish; each module that failed is logged and spoken (section 14) | the newer rules replace the held ones, a newer version having arrived before the first one's code (Dickson, 2026-11-09); publish; failures spoken; the first version's code, if it comes, is stale | to `AwaitingCode` with the new rules; nothing published, the current version stays everywhere | the newer rules replace the pending ones; nothing published | to `RulesReady` (a reload in developer mode, which the loader sends only then; `extensions.md` 2.2's "to `Loading`" passes through at once); publish; failures spoken | dropped; the newest version seen is updated |
| 2. Rules result, newer version, failed | to `Failed`; publish (nothing of it); spoken | stays `RulesReady` with the held version, whose tables stay published and whose code is still awaited, since a failed newer version leaves the previous version everywhere (`extensions.md` 2.2; Dickson, 2026-11-09); nothing published; spoken as a failed reload | stays `Idle` with the current version; spoken as a failed reload (build B's sentence) | back to `Idle`, the pending rules discarded; spoken as a failed reload | stays `Failed` with the new reason; spoken as a failed reload | dropped; the newest version seen is updated |
| 3. Code result for a version | dropped as stale: `Loading` holds no rules, so this is a late result of a version disabled before its code came | for the held version, successful: to `Active`, `Idle`; publish, adding hook registrations (none in build A); modules whose code failed are removed with their rules and spoken. For the held version, failed: to `Failed`, since no earlier version of this extension was ever active; publish, its tables no longer published; spoken. For another version: stale, dropped | dropped as stale: the current version's code was taken, and no other version's rules are held | for the pending version, successful: to `Active`, `Idle`, at the new version, its rules and hooks published together; `turned_off` emptied, since the new version may fix what the breaker stopped (Dickson, 2026-11-09). For the pending version, failed: back to `Idle`, the previous version everywhere; spoken as a failed reload. For another version: stale, dropped | dropped as stale: `Failed` holds no rules | dropped; the newest version seen is updated |
| 4. The start gate: is a publication made | no, for the built-in extension: no generation is published while it is `Loading`, so no child starts (section 9.1); another extension's `Loading` does not hold it | yes | yes | yes | yes | yes |
| 5. What is published of it | nothing | its tables, none of its hooks | its tables, and the hooks of modules not turned off | as `Idle`, the current version | nothing | nothing |
| 6. The user disables it (build B's dialog) | to `Disabled`; a result in flight arrives in `Disabled` and is dropped there (row 1, 2 or 3), or, after a later enable, is taken in `Loading` as any newer result and replaced by the enable's own load (row 1, `RulesReady`) | to `Disabled`; publish | to `Disabled`; publish | to `Disabled`, the pending rules discarded; publish | to `Disabled` | nothing |
| 7. The user enables it (build B) | nothing | nothing | nothing | nothing | nothing | to `Loading`; `Effect::Load` to the loader |
| 8. The breaker's verdict on a module (build B) | dropped: none of its hooks is published, so this is a late verdict from before a disable | dropped: no hooks are published | built-in or developer folder: the module joins `turned_off`; publish. Installed: to `Disabled { by: breaker }`; publish | the same as `Idle`, the pending version kept | dropped: none of its hooks is published | late, dropped |
| 9. The host ends (build B) | nothing | nothing; a compile in flight is answered by the loader as a failed code result (row 3) | nothing in the record; calls in flight count by row 8's rules | the same | nothing | nothing |
| 10. A stale judgement or effect names one of its modules | effects dropped, as late effects of a `Loading` extension are (Dickson, 2026-11-09); judgements used | used | used, unless the module is turned off, when effects are dropped | the same as `Idle` | effects dropped; judgements used | effects dropped; judgements used |
| 11. A module command names one of its modules (build B) | dropped | dropped: no bindings are published | run, unless the module is turned off | run under the current version | dropped | dropped |
| 12. Verbatim+F1 lists it | "loading" | each module "rules ready" | each module "active", or "turned off" | each module "active, version <n> pending" | "failed: <reason>" | "disabled" |

`Loading` cannot hold the start for ever on a loader that stops: every load is answered, by its result or by its token's drop (section 7.2). Only a read that hangs holds it (section 17).

### 8.3 Publication

On each publication the reducer mints the generation and builds a `Publication`: the generation; the listener's slice, every identity's listener-placed rules; each identity's outpost slice; the fetches; Core's own slice; and the gesture map (section 10). Inside Core each slice is lists of `Arc<ModuleRules>`, so a publication copies no table whatever the number of rules. A theme change that changes the fetches also publishes, so the fetches carry the generation too (roadmap M5, "Configuration versioned"; ownership, "the fetch configuration is sent unversioned until M5").

The rules' representation. `Input`, `Effect` and the reducer's state derive `Clone`, `Eq` and `Serialize` (`verbatim-model`'s `event.rs`, `verbatim-core`'s `state.rs`), because the flight recorder clones the state at each checkpoint and writes inputs and state into dumps; `regex::Regex` is neither `Eq` nor `Serialize`. So the rules Core carries are data: `Matcher`, `Property`, the three rule types, `ModuleRules`, `RuleReference`, `Generation`, the slices and `Publication` are `verbatim-model` types, `Clone`, `Eq` and `Serialize`, a regular expression held as its source text and compared by it. `verbatim-ext-api` depends on `verbatim-model`, never the reverse: it parses manifests into these types, validates and places them, compiles them, and evaluates them. The loader checks every regular expression compiles, so a module with one that does not is refused at load (section 5.4).

Regular expressions are legal only in node rules (section 5.2), so only the readers that apply node rules compile any: an outpost, for its identity's node rules, and the listener, for listener-placed vetoes. Each turns the slice it receives into its compiled form once, at its boundary, as it takes the configuration (parse, don't validate): the listener's I/O thread before it stores the slice (section 9.2), the outpost's I/O thread before it posts the configuration to its reducer. The compiled form is a `verbatim-ext-api` type, which the outpost's reducer carries to its programs, so `verbatim_ext_api` joins the outpost reducer's allowed paths (`xtask/src/ownership.rs`, `REDUCER_ALLOWED`) as a reviewed change; the reducer only holds it, and the worker evaluates it. A slice whose expression fails to compile in a reader cannot come from Core, whose loader compiled it; it is treated as the reader treats any message it cannot decode. Core evaluates window and event rules only, which take no regular expression, straight from the data. The cost: one compile of each regular expression in a reader's slice at each publication that reader receives. The built-ins have none, so build A's cost is nil; build B records the figure when modules with regular expressions exist. Across a pipe a slice is serialized: its bytes per publication are recorded (section 15), and publications come only at start, at a theme change and, in build B, at a reload.

The reducer stays pure: the publication is an effect, and loader results are recorded inputs of plain data, so the flight recorder replays them.

## 9. Step 3: configuration at a generation in each reader

### 9.1 Start-up, and the first command of every child

No reader judges under no configuration (Dickson, 2026-11-09), but the reducer thread, which mints the first publication, needs the supervisor in its context (`ReducerContext::supervisor`, used by focus requests, views, fetches, live re-reads and the idle barrier). So the supervisor is built first, as today, and its owner thread starts in an explicit phase in which it launches nothing (section 13, table 3). The order, on the main thread unless named:

1. `Supervisor::new` starts the owner in `AwaitingFirstPublication { wanted }`. The owner records the identity `verbatim` for Core's own process id in its application record (section 4.2). It launches no listener, spare or outpost.
2. `warm_own_outpost` asks for Core's own outpost, as today; the owner holds the pid in `wanted`.
3. The reducer thread, the GUI events thread and the control server start, as today.
4. `Input::Started` is posted (the start sound), then the loader thread is started (section 7.2), then `FocusNow` for the foreground application is posted, as today; the shell's `want_focus` records the wanted focus in `wanted.rs` as it does for any application whose outpost is not ready, and calls `ensure_spawned`, which the owner holds in `wanted`.
5. The main thread waits for the first publication on a one-shot channel the reducer's shell sends it on at the first `Effect::Publish`; if the reducer thread ends first, at a shutdown, the channel closes and the main thread goes on to shut down. It then starts the keyboard hook with that publication's gesture map (section 10), and then runs the GUI loop. Until then keys pass to applications untouched, as they do before Verbatim starts; no GUI command can be made, since the GUI loop is not running.
6. The reducer takes the built-in extension's rules result and publishes. Its shell posts `OwnerEvent::Published`; no child is live, so nothing else is sent.
7. The owner, on its first `Published`, moves to `Running { publication }`: it launches the listener, whose first command is `CoreToListener::Configure { generation, slice, own }`, `own` naming Core's process id and the identity `verbatim`; then, for each pid in `wanted`, Core's own with its stated identity at once, every other through `Identifying` (section 4.2), whose `Identify` is written on the listener's pipe behind its `Configure`. Spares start once Core's own outpost is ready, as today (`keep_spares`).

Before the first publication the reducer's shell receives no key (the hook is not started) and no GUI command; a focus wanted waits in `wanted.rs` for its outpost, as at any start; Core's idle barrier finds no child, as the owner already answers a round with no listener (`idle_round`); a control-plane request that needs an outpost is handled as today before any outpost is ready. No live re-read can be asked, since no focus has been reported.

The owner writes each child's slice as that child's first command, before any fact is released to it:

- the listener: `CoreToListener::Configure { generation, slice, own }`; the listener reads this first command before it builds `Listener` and installs its hooks and subscriptions (`run_listener`, which today builds `Listener::new` before reading a command), so it forwards no fact before its configuration;
- an outpost launched for its application: `CoreToOutpost::Configure`; `run_pipe` reads this first command before it calls `Outpost::assign`, which sets the outpost's application and installs the process's `WinEvent` hook; until then its target is pid 0, as a spare's is, so the UI Automation subscriptions `Outpost::build` registers judge none of its application's events before its configuration;
- a spare given an application: `CoreToOutpost::Assign { target_pid, identity, configuration }`. A spare builds its reducer and worker at launch, as today, so that its warm-up is not lost, but it judges no event before `Assign`: it has no application (its target is pid 0, which owns no window), and its hook is installed by `assign`, which posts the configuration to the reducer before it installs the hook.

So each reader's configuration is part of what it holds about its application from the moment it has one: the listener from its construction, an outpost from `assign`. A spare before `Assign` is today's spare state, in which no event is judged (section 13, table 2).

The owner's copy of the publication is a copy of a fact; it is kept because a child's first command must be written by the thread that releases the child's held facts, or a fact could pass the configuration. Its staleness is recognised by generation: each outpost reports the generation of its first command (`OutpostMessage::Started` names it) and the app keeps the generation sent to each live outpost, while the owner, which writes every configuration the listener receives, knows the listener's; after each input the app sends the current generation to any live outpost that holds an older one, and the owner sends it to the listener at each `Published` (section 9.2). A reader ignores a configuration not newer than the one it holds.

### 9.2 Later generations

On `Effect::Publish`, the app shell, on the reducer thread:

- posts `OwnerEvent::Published` to the supervisor's owner, which keeps the newest publication and writes `CoreToListener::Configure` to the listener when the listener holds an older generation; the owner wrote the listener's first command, so it is the one writer of the listener's configuration and knows what the listener holds;
- sends each live outpost `CoreToOutpost::Configure` with its identity's slice and the fetches, replacing today's `CoreToOutpost::Fetches`, when the generation that outpost was last sent is older than the current one. The app keeps that generation in its record of each live outpost, starting from the generation `OutpostMessage::Started` names, in place of today's `fetches_sent` (`live.rs`), and makes the same comparison after every input, as it does for the fetches today. So a full queue is retried after the next input, and an outpost whose first command carried an older generation than one published while it started (its `Started` reaching the reducer thread after the publication) is brought up to date at the next input; meanwhile its judgements name the older generation, which Core recognises;
- stores the gesture map in the hook thread's view (section 10).

In the listener, the I/O thread compiles the slice (section 8.3) and stores it in an `ArcSwap` (the `arc-swap` crate the hook thread's view already uses), which each reading thread loads once at the start of judging each event. This is not hand-written concurrency: one writer, many readers, through a well-tested crate's one operation.

In the outpost, the I/O thread compiles the slice and posts the rules to the reducer's mailbox (`Posted::Configure`), and the reducer applies them between two inputs. The fetches have readers outside the reducer: the gates of reads and text reads (`context.fetches()` in `outpost/mod.rs`, `read.rs`, `text_reads.rs`), the MSAA registry (`verbatim-ia2`, `registry.rs`) and the store (`store.set_fetches`). They keep reading the copy the I/O thread writes, under today's lock, as today; the I/O thread now writes it with its generation in the same step in which it posts the rules, and a read names the generation of the fetches it read. Moving those readers onto the configuration each program carries would remove the copy, and is not part of build A (section 17).

## 10. Step 3: the gesture map at a version

The decision machine (`verbatim-input`, `DecisionMachine`) stops owning its map. The hook thread loads `Arc<PublishedGestures>` from an `ArcSwap` once per key and passes it to the machine with the key, so the machine stays pure. The hook is started by the main thread with the first publication's map (section 9.1, step 5), so the view is never empty and no generation 0 exists.

`PublishedGestures` holds its generation; Core's bindings, observed and held gestures (today's `GestureMap`); Core's gesture-to-script table (today's `key_reports::Tables`, built apart from the map at start in `verbatim-app`), so that the report callback translates a key with the same published value the machine resolved it with, passed to it with the key; and module bindings by scope: global, and per identity, each binding a set of keys, a number of presses, and the binding it names (extension, module, command, declared gesture). In build A the module part is empty in production, because the loader refuses commands.

On a press, the machine counts presses of the same keys as it does today (`count_press`), then resolves among the bindings whose scope is active, by layer first and, within a layer, global before app (Dickson, 2026-11-09):

1. the developer folder's or installed extensions' global modules, then their app modules for the identity in Core's `HookForeground` view;
2. the built-in global modules;
3. the built-in app modules for that identity;
4. Core's commands.

An app module's scope is active only while the view's window is still the foreground window, as the view is read today. So an installed Notepad module binding Verbatim+F11 runs in Notepad, and the tray list, a built-in global module from step 8, opens everywhere else. Among one scope's bindings of the keys, it takes the one bound to the count, or else the highest count below it (the commands page). It stamps the raised gesture with:

- `generation`, of the map it resolved with;
- `presses`, the count;
- `resolved`: Core's script, or the module binding with its scope (global, or the identity it was resolved for);
- `hold_until`: the press's time plus the multi-press timeout, only when a next press of the same keys, resolved with the same map by the same order, would resolve to a different binding than this press did; otherwise none. So a binding of a higher count in a lower layer that the higher layer's binding hides holds nothing, since the next press would resolve to the same binding again.

A key is swallowed when any active scope binds it, as today when Core's map does. Core's own multi-press scripts (report current object, the time and date, the tray list) stay bound at one press and keep dispatching on the count, so nothing they say changes. The hook thread adds one lock-free load per key and an ordered-map probe per active scope; it takes no lock and allocates nothing more than today's report.

## 11. Step 3: the log ring, the viewer and Verbatim+F1

### 11.1 The ring

A `tracing` layer, beside today's formatter and error-sound layer, appends each formatted line at or above the configured level to a ring of the newest lines (2,000 lines), with a monotonic sequence number per line. The ring is a monitor: one module, one private mutex held only for the append of one line or the copy of the ring, never across a call (ownership, "One owner per fact"). It is a lock that the threads that log take, the reducer and the synthesizer thread among them, whenever they log at the configured level; its hold is one line's append, and the GUI thread's copy of up to 2,000 lines when the viewer opens, measured (section 15). The hand-off cannot be removed: the ring is written by many threads and read by the GUI thread, and a channel to a ring the GUI thread owns would wake the GUI thread on every line, or, left undrained until the viewer opens, keep the oldest lines rather than the newest. The threads that log write it; the hook thread and the audio thread never log, which the ownership check already enforces for the hook. Core's file stays as it is: the formatter's output, which the launcher writes to Core's log. No thread is added.

### 11.2 The viewer

A frame on the GUI thread, "Verbatim Log Viewer", holding a read-only multi-line text control, as NVDA's (`gui/logViewer.py` 27 to 46). `GuiCommand::OpenLogViewer { at }` opens it, or brings it to the front, with the ring's lines from the oldest kept, and puts the caret at the start of the line numbered `at`, or at the end when `at` is none; Escape closes it, as in NVDA. The Verbatim menu gains "View log", between Settings and Exit, which opens it at the end. States: section 13, table 6.

### 11.3 Verbatim+F1

`ScriptAction::DeveloperInfo`, bound to Verbatim+F1 in both layouts. The reducer asks the navigator's outpost `Query::DeveloperInfo { node }`, which the worker answers with the node read afresh with every detail, its provenance, and the generation, through the query's one answer path (Core's request table: the answer, "gone" at the outpost's end, "failed" at the deadline). On the answer, the reducer emits `Effect::LogDeveloperInfo`, whose shell appends the record to the ring directly, whatever the configured level, taking its sequence number from that append (`tracing` returns none), writes it to Core's log file under a target the ring's layer skips, so the ring holds it once, and then sends `OpenLogViewer` at that sequence number. The record, in English as NVDA's is:

- "Developer info for navigator object:" (NVDA's words, `globalCommands.py` 3286 and 3287);
- name, role, value, states, description, and every `NodeDetails` field present, each with its provenance where a rule set it;
- the backend, node id and outpost generation;
- the application identity;
- the generation the outpost judged under, and Core's current one;
- the modules whose rules apply to the identity, by layer, with their state (section 8.2, row 12).

With no navigator, the record is "No developer info for navigator object" (`globalCommands.py` 3290 and 3291). When the query is answered gone or failed, the record says so, with Core's own facts, and the viewer opens. As for any query, nothing is said while the application does not answer. The record holds no matcher; a matcher is printed on demand by `verbatim-inspect matcher` (Dickson, 2026-11-09).

### 11.4 Authoring tools on demand

Three `verbatim-inspect` commands, run only when asked, through the control protocol (Dickson, 2026-11-09):

- `verbatim-inspect matcher [--navigator]` prints a ready-made matcher for the focus, or the navigator object, in the module manifest's TOML form: the identity as `applications`, the backend, and a `match` table of every matcher property the node has except `name` and `runtime-id`, which change with the language and between runs (decided here).
- `verbatim-inspect dump-tree --properties` adds to each node of today's dump every matcher property it has, written with the matcher's own names (`uia-class-name = "..."`), so a line can be copied into a matcher.
- `verbatim-inspect match '<matcher>'` lists the nodes of the foreground application's tree that the matcher selects, each as `dump-tree` prints it. The text is parsed once, by Core's control server through `verbatim-ext-api`, against node-rule legality; a parse error is the answer, with the parser's message. The outpost evaluates the parsed matcher over the same walk as `dump-tree`.

Each runs as a query on the outpost of the focus, the navigator or the foreground window, through Core's request table's one answer path, as `dump-tree` does today.

## 12. Messages added

Between processes, each with its kind (`verbatim_model::protocol::Kind`):

| Message | Direction | Fields | Kind |
|---|---|---|---|
| `CoreToListener::Configure` | Core to listener | `generation`, the listener slice: for each identity, its window rules (`normalize-class`, `backend`), its listener-placed vetoes, drops and accepts, as `verbatim-model` data; `own`, Core's process id and the identity `verbatim` | Configuration |
| `CoreToListener::Identify` | Core to listener | `pid`, `client: Option<Pid>` (a console host's client, when the app knows it) | Ask, answered by one `Process` |
| `ListenerToCore::Process` | listener to Core | `pid`, `identity` | Lifecycle |
| `ListenerFact`, changed | listener to Core | gains `generation` | Routed |
| `DeliveredFact::UiaNotification`, changed | inside `ListenerFact` and `DeliverFact` | gains `accepted: Option<RuleReference>` | |
| `CoreToOutpost::DeliverFact`, changed | Core to outpost | gains `generation`, copied from the `ListenerFact` | Routed |
| `ListenerToCore::Foreground`, changed | listener to Core | gains `identity` | Report |
| `UiaSnapshotFact`, changed | inside facts | gains the cached fields of section 3.1 | |
| `CoreToOutpost::Configure` | Core to outpost | `generation`, the identity's rules, the fetches; replaces `Fetches` | Configuration |
| `CoreToOutpost::Assign`, changed | Core to spare | gains `identity` and `configuration` | Lifecycle |
| `OutpostToCore::Event`, changed | outpost to Core | gains `generation` | Report |
| `NormalizedEvent::Notification`, changed | in an outpost's event | gains `accepted: Option<(RuleReference, Generation)>`, the listener's verdict | |
| `Query::DeveloperInfo` and its `QueryResult` | Core to outpost and back | `node`; the answer's snapshot, provenance, window facts and generation | Ask and Answer |
| `FocusNowOutcome::Vetoed` | in a focus-now answer | `rule`, `generation` | Answer |
| `NodeDetails`, `NodeSnapshot`, `StateSet`, changed | in every snapshot | section 3 | |
| `WindowFacts`, changed | in focus reports | loses `application`; `class` is the class as rules see it; gains `root_class` | |
| `Query::Matcher` and its `QueryResult` | Core to outpost and back | `node`; the answer's matcher properties and backend | Ask and Answer |
| `Query::DumpTree`, changed | Core to outpost | gains `properties: bool` | Ask |
| `Query::Match` and its `QueryResult` | Core to outpost and back | the parsed `Matcher`; the answer's matching nodes | Ask and Answer |
| Control `Request::Matcher`, `Request::Match`, `Request::DumpTree` changed | `verbatim-inspect` to Core | `navigator: bool`; the matcher's text; `properties: bool` | control protocol |

Within Core:

| Message | From and to | Fields |
|---|---|---|
| `Input::ExtensionRules` | loader to reducer | `extension`, `version`, result: the parsed modules as `verbatim-model` data and failed modules, or a reason (the loader's own stop included, section 7.2) |
| `Input::ExtensionCode` | loader to reducer | `extension`, `version`, result |
| `Effect::Load` | reducer to loader (build B's producers) | `extension` |
| `Effect::Publish` | reducer to its shell | `Arc<Publication>` |
| `OwnerEvent::Published` | app to supervisor owner | `Arc<Publication>` |
| `OutpostMessage::Started`, changed | supervisor to app | gains `identity` and the `generation` of the outpost's first command |
| The first publication | the reducer's shell to the main thread, once, on a one-shot channel | `Arc<Publication>`, whose gesture map the hook is started with |
| The raised gesture, changed | hook thread to reducer | gains `generation`, `presses`, `resolved`, `hold_until` |
| `GuiCommand::OpenLogViewer` | app to GUI thread | `at: Option<u64>` |
| `Effect::LogDeveloperInfo` | reducer to shell | the record's lines |

Every enum's `kind()` and the unit test listing each variant with its kind are updated, so each new variant changes a reviewed file.

## 13. States added, counted

Ten lifecycle states are added, in five tables; each is listed with why it cannot be removed.

### Table 1: the extension lifecycle (reducer), six states

`Loading`, `RulesReady`, `Active` with `Idle` and `AwaitingCode`, `Failed`, `Disabled`: section 8.2 is their table. Justification: `Loading`, `RulesReady`, `Active`, `Failed` and `Disabled` are the decided table (`extensions.md` 2.2). `RulesReady` exists because rules publish before code (Dickson, 2026-11-09). `AwaitingCode` is the reload's "keeps both new results until both arrive"; without it a reload would publish new rules beside old hooks. `Failed` and `Disabled` are separate because only a reload leaves the first and only the user's enable the second. No state carries a flag: staleness is the record's newest version, which is valid in every state.

### Table 2: each reader's configuration, no state added

Not a lifecycle: one value, replaced by a newer generation and never absent once the reader has an application to judge, since the listener and a launched outpost read it before anything else and a spare receives it with its application. The rules that read it:

| Rule | Listener | Outpost launched for its application | Spare | Hook thread |
|---|---|---|---|---|
| Before its first configuration | nothing runs: `run_listener` reads its first command before it builds `Listener` and installs its hooks | nothing runs: `run_pipe` reads its first command before `assign` installs the hook | today's spare state, with no application: no hook, target pid 0, so no event is judged | the hook is not started (section 9.1) |
| Its first configuration arrives | the listener is built from it | `assign` posts it to the reducer, then installs the hook | with `Assign`: posted to the reducer, then the hook installed | the hook is started with it |
| A configuration newer than the one held arrives | compiled and stored; each reading thread takes it at its next event | compiled, posted to the reducer, applied between two inputs; the arbitrator's kept verdicts dropped | none can arrive: the app sends configurations only to live outposts, and a spare is not one until `Assign` | stored by Core's shell; the next key takes it |
| One not newer arrives | ignored | ignored | none can arrive, as above | none can arrive: one writer |
| An event is judged | under the slice loaded at its start; the fact names its generation | under the slice its program carries; results name it | none is judged | under the map loaded for the key; the gesture names it |

### Table 3: the supervisor owner's phase, one state added

`AwaitingFirstPublication { wanted }`, then today's running owner, `Running { publication }`. Justification: the reducer thread needs the supervisor before it can mint the first publication (section 9.1), and no child may start before that publication, so the owner exists for a time with nothing to launch; the alternative, building the supervisor after the first publication, leaves the reducer thread with no supervisor for the focus requests, views and idle rounds it handles from its first input. `wanted` holds the pids asked for meanwhile, so a request made before the publication has its answer after it.

| Rule | `AwaitingFirstPublication` | `Running` |
|---|---|---|
| `Published` | to `Running`: the listener launched with `Configure` first; Core's own pid in `wanted` started at once with its stated identity; every other pid in `wanted` to `Identifying` (table 4), its `Identify` behind the listener's `Configure` | the newest kept; `Configure` written to the listener when it holds an older generation |
| `EnsureSpawned` | the pid joins `wanted`; a pid already there, nothing | as today; Core's own pid has its stated identity, and a pid with no identity enters `Identifying` |
| `Views`, `BackoffClock` | stored, as today | as today |
| A message read from the listener (`Fact`, `ForwardingEnded`, `MenuOrSwitchEnded`, `Desktop`, `Foreground`, `Process`) | none can arrive: no listener is launched | as today, and `Process` as section 4.2 says |
| A message about a child (`Ready`, `Pong`, `Unreadable`, `Launched`, `Retired`) | none can arrive: no child is launched | as today |
| `IdleRound` | answered as `idle_round` answers today with no listener and no outpost | as today |
| `LiveReread` | none can arrive: Core asks a re-read only of a focus reported to it, and no child has reported one | as today |
| `Shutdown` | the summary sent at once, empty, since no child exists; the owner ends | as today |

### Table 4: the supervisor's `Identifying`, one state

An application's record before its identity is known, entered only when an outpost is wanted for a process other than Core's own that the listener has not reported. Justification: the identity has one resolver for every process but Core's own, the listener, and an outpost's assignment needs its identity, so a start with no fact must wait for the answer; the alternative, starting the outpost and sending its identity later, gives the outpost an unidentified state in which its rules cannot apply.

| Rule | `Identifying` |
|---|---|
| The listener's `Process` for the pid, whether answering `Identify` or sent ahead of a fact | the identity is kept, `Unknown` included; the process is held; the outpost is started or given a spare, as a start is today |
| A fact for the pid | cannot arrive first: the listener sends `Process` ahead of every first message naming a process (section 4.2), so the record has left `Identifying` before the fact is read |
| Another `ensure_spawned` for the pid | nothing: one request is waiting |
| The process exits (the sweep finds it) | the record ends; the app hears `NotWatched`, as for any start of an exited process |
| The listener ends or is replaced | the request is sent again to the new listener once it starts |
| Verbatim shuts down | the record ends with the rest |

### Table 5: the listener's process record, one state (`Present`)

Absent, or `Present { handle, creation_time, identity }`, kept in the listener's outgoing backlog under the transport's lock (section 4.2). Justification: the record is the decided cache keyed by the process (`extensions.md` section 3); its open handle is what keeps a reused process id from inheriting an identity, and it is the one place the order of `Process` before a process's first fact is decided.

| Rule | Absent | `Present` |
|---|---|---|
| A message naming the pid is sent, by any thread | the kernel calls made; when the process opens, the record created and `Process` pushed ahead of the message; when it does not, `Process { pid, Unknown }` pushed ahead of the message and no record kept | the message pushed; a `Foreground` takes its identity from the record |
| A reading thread reads the identity to judge an event | `Unknown`: the process could not be opened, or its record ended at `ApplicationExited` after its marker was sent | the record's identity |
| `Identify { pid, client }` | as a message naming the pid, with `client` naming a console host's client; the pushed `Process` is the answer | `Process` pushed from the record as the answer |
| The first `Configure` names Core's own process | the record created with the stated identity `verbatim` and the process's handle, and no `Process` sent | none can arrive: the first configuration is read before any message is sent, and later configurations' `own` is ignored |
| `ApplicationExited` | answered `ExitHandled`, as today | the record removed and its handle closed; answered `ExitHandled` |
| The listener ends | nothing | the record ends with it; a replacement starts with none |

### Table 6: the log viewer, one state (`Open`)

| Rule | No viewer | `Open` |
|---|---|---|
| `OpenLogViewer { at }` | created with the ring's lines, caret at `at` or the end; to `Open` | the text replaced by the ring's lines, caret moved, brought to the front |
| The user closes it, or presses Escape | none can arrive | destroyed; no viewer |
| Shutdown | nothing | destroyed |

Counted: six in table 1, one in table 3, one in table 4, one in table 5, one in table 6: ten. The authoring tools add none: each is a query with the request table's one answer path. The outposts and the hook thread gain no state: a configuration is a value each holds once it has an application (table 2). No set of warned rule pairs is kept anywhere (section 5.4).

## 14. What is spoken

| When | Speech | Source |
|---|---|---|
| A built-in module fails to load, its rules or its code | "Built-in module <folder name> failed to load", once, at once and unscheduled (Dickson, 2026-11-09, kept until M8.6), after the start sound, so the first focus announcement that follows usually cuts it; plus the error sound, since it is logged as an error (`error_sound::ErrorSoundLayer`); a Windows toast replaces it in M8.6 | `extensions.md` 2.2; Dickson, 2026-11-09 |
| The built-in folder cannot be read at all, or the loader stops (section 7.2) | "Built-in modules failed to load", at once and unscheduled; a toast in M8.6 | Dickson, 2026-11-09 |
| File Explorer's snap results, from a File Explorer that is not the focus's application | spoken, as today and as NVDA speaks them, from the verdict the listener carries (section 6.5) | `explorer.py` 519 to 535 |
| An MSAA object reports checked and half checked | "half checked", where Verbatim says "checked half checked" today | NVDA, `NVDAObjects/IAccessible/__init__.py` 1141 and 1142 |
| An MSAA object reports expanded and collapsed | "expanded collapsed", unchanged, confirmed against a capture before it is built | NVDA, `controlTypes/state.py` 56 and 57; Dickson, 2026-11-09 |
| An installed app module and a built-in global module bind the same keys | the app module's command, in its application; the built-in's elsewhere (section 10) | Dickson, 2026-11-09 |
| Verbatim+F1 | the log viewer's focus announcement, its window title "Verbatim Log Viewer" in place of NVDA's "NVDA Log Viewer", then its edit and the line at the caret, "Developer info for navigator object:"; the scenario's exact expectation is taken from a capture of NVDA+F1 in Notepad with the pinned build, the product name replaced | `globalCommands.py` 3266 to 3291; `gui/logViewer.py` 27 to 46 |
| The Verbatim menu | the new item "View log" | NVDA's Tools menu item "View log" |
| Every Explorer and Task Manager rule moved | unchanged | pinned by the scenarios and by Core's unit tests moved to the rule tests |

## 15. Costs on the event and speech paths, and how they are measured

Added on the event path:

- **Listener, each UIA focus and menu opening:** one `ArcSwap` load, one brief hold of the transport's existing lock to read the identity from the record (`Sender::inspect`), one ordered-map probe for the identity's rules, and one probe per layer for vetoes keyed by property, then the candidates' matchers: well under a microsecond for the built-ins' few rules. No call or wake-up added, and no new lock.
- **Listener, each selection or notification:** the same probes for drops and accepts; a `GetClassNameW` only when a rule of that identity names the class and the class was not read already.
- **Listener, each new process:** the identity's kernel calls once, made under the transport's existing lock (section 4.2), so every other sender waits for them that once, and one `Process` message. A process that cannot be opened pays an `OpenProcess` attempt and a `Process` message per message naming it.
- **Outpost, each snapshot:** the node rules of its identity, a few comparisons each. MSAA: one `GetDlgCtrlID` per snapshot of a child window, a local call. UIA: four or five more properties in existing cache requests, no added round trip. Bytes: the added fields per snapshot, recorded in the ledger.
- **Outpost, each focus:** the vetoes; a redirect adds the read of its target, one call or one `FindFirst`, only where a rule asks for one.
- **Core, each focus report:** one probe for `silent`.
- **Hook thread, each key:** one `ArcSwap` load and one probe per active scope.
- **Outpost, each snapshot with no ancestor chain, only for an identity whose rules name `parent-automation-id`, and only once every other property of the rule matched:** one parent read; Task Manager is the only built-in that names it, and it cannot be read before M8.7.
- **Each reader, each publication it receives:** the slice's bytes across the pipe, and one compile of each regular expression in it, none for the built-ins.
- **Startup:** the built-in rules' load before any child starts.

The authoring tools are not on the event path: `match` and `dump-tree --properties` walk the foreground tree, a cost that grows with the tree, only when a developer runs them.

Locks. Two holds touch the event path, and both are stated with why the hand-off stays. The listener's process records live under the transport's existing lock, which every message to Core already takes; the hold grows by a map probe per message and by the identity's kernel calls once per process, and the alternative, a separate owner of the records, would need a second lock to keep `Process` ahead of the first fact (section 4.2). The log ring's mutex is new: the reducer, the synthesizer thread and every other Core thread that logs take it whenever they log at the configured level, for one line's append, and the GUI thread takes it to copy the ring when the viewer opens; the hook and audio threads never take it (section 11.1). No wake-up is added per event: configuration messages are sent only at a publication.

Measured inside the build, before and after, by the existing tools (CLAUDE.md, "Measure each build's wake-ups and processor time before and after"):

- wake-ups and processor time per process, `cargo xtask trace scenario` with `--after 5 --seconds 20` on `measure_focus_windows_terminal` and `measure_focus_new_application`, compared with the ledger's figures of 2026-11-08;
- latency, `cargo xtask measure` on the whole measure set, key to speech queued, with the listener's handling of a focus;
- the call ledger: the rules-only built-ins leave every count unchanged, and an MSAA focus adds exactly one `GetDlgCtrlID` per MSAA snapshot of a child window, pinned by a call-count test;
- bytes per focus report before and after, from the flight recorder;
- the time from Core's start to the first publication, to the listener's `Ready`, to Core's own outpost's `Ready` and to the first spare's, over ten starts with a warm disk and three with a cold one, before and after, recorded in the ledger (Dickson, 2026-11-09), since Core's own outpost exists so that Verbatim's own menus are read at once (`warm_own_outpost`);
- the listener's transport-lock hold for a new process's kernel calls, and its longest hold per minute in `measure_focus_new_application`;
- the log ring's append time on the reducer thread while the GUI thread copies a full ring, and the copy's own time.

The performance ledger (`books/dev/src/reference/performance.md`) records the listener's and the outposts' per-event costs and the startup figure, since they are on the event path or at start; the loader's own time is not recorded unless it is measured slow.

## 16. Tests

All by `books/dev/src/reference/testing.md`: one fixed target and code path, exact assertions, waits only for evidence, no branching on the environment, no alternative expectations, no substring matches.

Unit tests:

- `verbatim-ext-api`: one test per refusal of section 5.4, each asserting the exact message; the matcher's parser on every form, and its refusal of each illegal form per use; evaluation of each form on present and absent properties, `parent-automation-id` included; the refusal of `parent-automation-id` in an MSAA rule and of `containers` on anything but `"none"`; the version folding; the loader's warning for two node rules of one module setting one property differently; placement of each rule kind from its properties; a slice compiled from its data form, and the data form's round trip through serialization.
- `verbatim-reader-rules`: `class_verdict` with rules in NVDA's order, one case per row of section 6.4; `normalize-class` seen by a listener veto naming `window-class`.
- `verbatim-listener-logic`: the veto step's place in `next`, a veto's drop reason naming rule and generation, and no veto on a foreground change; the identity resolver, a pure function over the kernel calls' answers, one case per row of section 4.1 (`wwahost.exe` with and without a user model id, `msedgewebview2.exe` with its creator, with a parent created after it and with none, a console host with its client, a process that cannot be opened).
- `verbatim-outpost-logic` and `verbatim-outpost`: node rules applied in layer and written order with provenance, an overridden rule named in it; `containers = "none"` reporting a focus with no containers; `parent-automation-id` read from the chain for a focus and by one parent read otherwise, only after the rule's other properties matched, pinned by a call-count test; the `matcher`, `match` and `dump-tree --properties` answers on a fixed tree; a listener-placed veto applied to the outpost's own read after a menu ends and not to a listener's read; kept verdicts dropped at a new generation; a vetoed focus-now answer; a vetoed live re-read answered `LiveRereadGone`; a listener's class skip under another generation read by the outpost itself when its verdict is UIA, and a listener's read under another generation dropped when its verdict is MSAA; a notification's carried `accept` and the listener's generation copied from `DeliverFact` into the reported event; a launched outpost reading its first command before `assign`.
- `verbatim-core`: every cell of section 8.2's table, each as a replayed input sequence; the start gate; `silent`, and a notification accepted only by its carried verdict; Core's unit tests of the moved Explorer rules re-pointed at the compiled built-in tables (`a_uia_focus_on_the_task_switching_pane_is_refused_whole` and its siblings); state speech with the new `StateSet`, including "half checked" and "expanded collapsed", and the state-change diff from `Expanded` to `ExpandedAndCollapsed` speaking only "collapsed"; the failure sentences spoken at once, after the start sound and before a focus announcement queued after them, by exact text for a broken module, for an unreadable folder and for a stopped loader.
- `verbatim-core`'s supervisor policy: `may_start_outpost` for an accepted and an unaccepted notification.
- `verbatim-input`: the resolution order, one case per pair of adjacent layers and one for global before app within a layer, with the Notepad and tray-list sequence of section 10; a binding of two presses runs on the second press; a third press runs the highest count below; `hold_until` stamped only when the next press would resolve to another binding, not when a higher count is bound only in a lower layer the press's own binding hides, and not when a later map binds one; the gesture names the map's generation and the identity it was resolved for, and is translated by the script table of the same map; Core's scripts' repeat dispatch unchanged.
- `verbatim-app`: the ring keeps its newest lines with their sequence numbers; Verbatim+F1's record appended to the ring once at a level below the configured one, with its sequence number; the loader, given a temporary folder with one broken manifest, skips it and loads the others, with the exact error; the loader's token answering a load whose thread panics, and a later load answered at once once the thread has ended; table compilation of the built-in folders producing exactly the decisions the deleted constants made today, case by case; the outpost generation kept per live outpost from `Started`, and a newer one sent after the next input to an outpost whose `Started` named an older one.
- `verbatim-listener`: the process record's table (section 13, table 5), each cell; live identity resolution for the plain row and the console row against a test process and the test console; `Process` ahead of the first message naming a process when two threads send for it at once; no record kept for a process that cannot be opened; Core's own process recorded from the first configuration with no `Process`; the first command read before `Listener` is built; each listener-held property read from `ListenerEvent`.
- `verbatim-supervisor`: the owner's phase table (section 13, table 3) and `Identifying`'s (table 4), each cell; Core's own outpost started at the first publication with no `Identify`; `ApplicationExited` sent at once when holding a process from `Process` fails; a child's first command is its configuration, before any held fact; `generation` copied from `ListenerFact` into `DeliverFact`.
- `verbatim-inspect` and the control server: `match` with an unparseable matcher answers the parser's exact message; `matcher`'s output parses back into a matcher that selects the node it was printed for.

The generated test. Written by an agent other than the builder, committed with its world before the implementation and shown failing on known defects, run against a prototype before approval, every seed passing (CLAUDE.md, the generated-test rule). Its reference model is written by that agent from `extensions.md` 2.2 (its table, its rules and its sentence on how Core acts on a judgement from an older generation) and Dickson's recorded decisions, those quoted in section 0 and his answers of 2026-11-09 on this design's lifecycle cells, never from this design's tables (sections 6.6, 8.2 and 13) or its code. The world generates: loader results for several extensions with arbitrary versions, successes and failures (whole and per module), in any order and with stale ones, and a loader that stops; the user's enables and disables; breaker verdicts; host ends; theme changes; outposts started, assigned from a spare, replaced and ended; the listener replaced; focus, notification and selection events with properties from a small vocabulary; and every interleaving of configuration messages with events at each reader. After every step, the model and the system are compared on: each extension's state; the published generation and, per identity, which modules' rules are in force; each reader's held generation (never decreasing, never absent once it has an application); each judgement's outcome against the model's evaluation under the generation the judgement names; Core's action on each judgement by its kind; that no child started before the first publication and no first publication came while the built-in extension was `Loading`; and that a failed newer version left the previous version everywhere. Its size is a few hundred lines against a lifecycle of six states and three readers.

Integration tests against `mockapp` (`verbatim-outpost`'s tests, an outpost driven directly):

- the new snapshot fields filled from mockapp's UIA and MSAA controls, empty where the backend lacks them;
- a node rule setting a role, with the report's role and provenance;
- Task Manager's two rules, compiled from `extensions/builtin/taskmgr/module.toml` (planned) and filed under mockapp's identity, against mockapp's `TmViewRow` children and a `SysListView32`;
- a rule for another identity than the outpost's applies to nothing.

End-to-end scenarios, run by the parent from one background script, three times each at the end of the build:

- related scenarios unchanged: `explorer_folder_window`, `quick_link_menu` (the staging window), `alt_tab_switcher` (the switcher's veto and items, whether its rule moved to the module or stayed in Core);
- new, `explorer_snap_results`: in Notepad, with every other window minimized as the harness leaves them, Windows+Left snaps the window, and the snap results notification File Explorer raises, File Explorer not being the focus's application, is spoken; the exact expectation is taken from a capture of NVDA with the pinned build before the scenario is written. It is the one test that crosses every hop the carried `accept` takes (listener, supervisor, outpost, Core), which no other scenario covers;
- new, `developer_info_in_notepad`: Verbatim+F1 in Notepad, the viewer's speech from the capture, and the log record holding the edit's window class, `RichEditD2DPT` (the class NVDA's Notepad module tests, `appModules/notepad.py` 62), its UIA class name as read, and the identity `notepad`.

No end-to-end scenario asserts the start-up failure sentence. Whether the first focus announcement cuts it depends on how long Core's first outposts take to start against the sentence's length, a timing of Verbatim's own, for which the testing standard allows no optional utterance, and the sentence stays unscheduled (Dickson, 2026-11-09); a scenario that broke a module in the deployed built-in folder would also change the folder every scenario shares. So the failure path is pinned below the end-to-end level, with exact assertions: Core's unit test of the sentence's text and its place after the start sound and before a focus queued after it, and `verbatim-app`'s test of the loader over a temporary folder of its own with one broken module. Every scenario exercises the successful load from the deployed folder.

Because this build changes how Verbatim starts, the full suite runs once at its end (work plan, "the full suite runs once at the end of the loader step"). The test-coverage reference gains rows for Verbatim+F1, the View log menu item, the three authoring commands, each moved rule, the snap results, and the built-in failure sentence (unit and `verbatim-app` levels, with its end-to-end gap and the reason). Adding `explorer_snap_results` and `developer_info_in_notepad` is followed by `cargo xtask e2e-durations` once their GitHub run finishes.

## 17. Limitations

- Startup waits for the built-in rules before any child starts, Core's own outpost included; measured, and expected to be milliseconds with a warm disk. Keys pressed meanwhile reach applications untouched.
- A read of the built-in folder that hangs, rather than fails, holds the start, with only the start sound heard, until it returns; the guardian's start-up progress bounds it from M4.8.
- A console host's identity is its client's at the first message naming the host; a different program run later in the same console window keeps the first identity until the console host exits.
- A process that cannot be opened is `Unknown`, and pays an `OpenProcess` attempt and a `Process` message for each message naming it.
- If step 2's check finds no structure that tells the Alt+Tab switcher from Task View, `drops_ancestry` stays in Core until Dickson decides on `modifiers-held`.
- The built-in failure sentence is spoken unscheduled, so the first focus announcement at start usually cuts it; the log keeps it whole, and M8.6's toast replaces it. No end-to-end scenario asserts it (section 16).
- Task Manager's node rule matches the direct parent's automation id, where NVDA walks up through layout ancestors (`taskmgr.py` 18 to 21); it changes nothing spoken in build A (section 3.1); and Task Manager cannot be read end to end until M8.7, so its rules are proven against `mockapp` only.
- Module commands, hooks and components have no runner, and are refused at load, until build B.
- An outpost whose `Configure` meets a full queue, or whose first command was older than a generation published while it started, judges under the older generation until the next input, which its judgements name.
- The fetches stay copied outside the outpost's reducer, in the lock the gates read, the MSAA registry and the store, as today, each read naming its generation (section 9.2).
- `presentation` changes only which focus ancestors are spoken, since Verbatim's object navigation sees the whole tree.
- The log viewer shows the ring at the time it is opened or brought to the front, not lines logged while it stays open, as NVDA's refreshes only on activation and F5; F5 is not built.

## 18. What build B needs, and finds possible

The lifecycle's every transition and the stale-action table are built and tested; build B adds the producers (the host's results, the breaker, enable and disable, the developer folder and its reload). The publication's slices are lists of `Arc<ModuleRules>`, plain data, to which build B adds hook registrations per module; readers already compile regular expressions at their boundary, which build B's modules will first use. The loader's results are already rules then code. The gesture map already carries module bindings with scope, `hold_until` and the resolved identity, and the raised gesture names its generation, so Core can route an app module's command to the application the hook resolved, without judging the foreground again. The outpost's configuration is a reducer input, so its hook state (`NoHook` or `AwaitingHook`) joins the same reducer. Identity is in every outpost's assignment and in Core's per-outpost record, for the host's lanes. Nothing in build A puts a Wasm runtime in Core, the listener or an outpost.

## 19. Decisions taken here, for Dickson to know

Each is outside the areas Dickson owns, decided with its reason above, and reported in case it should be overridden:

- Window rules' `class` and `root-class` are the normalized classes, as NVDA's modules judge them (section 5.3).
- `role` is a listener-held property for veto placement (section 6.2).
- An absent property fails every positive matcher form (section 5.2).
- The File Explorer staging rule and the shell-root rule apply only in File Explorer's process, as NVDA scopes them, confirmed by step 2's dumps (section 7.3).
- The built-in extension still loads under `--extensions-off`, which turns off installed extensions and the developer folder (section 7.3).
- The log viewer's title, "Verbatim Log Viewer", and the menu item "View log", NVDA's wording with Verbatim's name.
- `NodeDetails` gains `control_type`, `msaa_role` and `presentation` beyond step 1's list, since the matcher and rule surface name them; `parent_automation_id` stays on every UIA focus, as today, and is read for other snapshots only for an identity whose rules name it (section 3.1).
- `parent-automation-id` matches the direct parent only (section 17).
- `verbatim-inspect matcher` leaves out `name` and `runtime-id` (section 11.4).
- The rules are `verbatim-model` data, compiled by each reader at its boundary; `verbatim_ext_api` joins the outpost reducer's allowed paths (section 8.3).
- The listener's process records live in its outgoing backlog, under the transport's existing lock, with the kernel calls made under it once per process; a process that cannot be opened keeps no record (section 4.2).
- The supervisor starts in `AwaitingFirstPublication`, and the main thread starts the keyboard hook only with the first publication (section 9.1).
- No conflict check between a module's rules: the later rule wins, and provenance names both (Dickson, 2026-11-09; section 5.4).
- A vetoed live re-read is answered `LiveRereadGone` (section 6.3).
- The outpost re-judges a listener focus fact made under another generation by its own backend verdict (section 6.3).
- `hold_until` is stamped only when the next press would resolve to another binding (section 10).
- No end-to-end scenario asserts the start-up failure sentence; `explorer_snap_results` is added (section 16).

## Decided by Dickson (2026-11-09)

- No child process starts before Core's first publication; the time this takes is measured and recorded (sections 9.1 and 15).
- `parent-automation-id` is a UIA node matcher property, and `containers = "none"` a node action that drops the focus's containers; Task Manager's rule uses the property (sections 5.2, 5.3 and 7.3). `alt-held` is rejected as too specific; the switcher rule waits on step 2's structural check (section 5.5).
- An MSAA object reporting both expanded and collapsed has the third value `ExpandedAndCollapsed`, spoken "expanded collapsed" as NVDA does, confirmed against a capture before it is built (section 3.2).
- "Built-in module <folder name> failed to load" is spoken at once, unscheduled; "Built-in modules failed to load" when the folder cannot be read; both become Windows toasts in M8.6 (section 14). The sentence stays unscheduled until then, and its test asserts no sentence the first focus cuts and does not change the shared deployed folder (section 16).
- Gestures resolve by layer first, global before app within a layer: the developer folder or installed extensions, then built-in global modules, then built-in app modules, then Core's commands (section 10).
- Authoring tools are on demand, not in every report: `verbatim-inspect matcher`, `dump-tree --properties` and `match '<matcher>'`; Verbatim+F1's record holds no matcher (section 11.4).
- `accept` is judged once in the listener and carried in the forwarded fact, and routing keeps the verdict and the generation to the outpost and Core (section 6.5).
- Verbatim's own identity is stated by Core: the supervisor records `verbatim` for Core's own process at start-up, and Core's first configuration to the listener names it; the listener records it rather than resolving it and resolves every other process (sections 4.2 and 9.1).
- The lifecycle cells: a reload clears the breaker's turned-off set; late effects from a `Loading` extension are dropped; newer rules replace ones held in `RulesReady`; a failed newer version leaves the previous version everywhere, as `extensions.md` 2.2 says (section 8.2).

## Open

- `modifiers-held`, only if step 2 finds no structural difference between the Alt+Tab switcher and Task View on Windows 11 (window class, root class, the list's automation id, the item's framework): a general matcher property saying which modifiers are held, which would let the switcher rule leave Core. Until Dickson decides it, the rule stays in Core, keyed by the identity `explorer`. If a difference is found, this point closes with no decision needed.
