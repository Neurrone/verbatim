# Extensions, designed for the rewrite

Status: design draft, Claude, 2026-11-28. Nothing in the repository changes because of it. For the rewrite it replaces `docs/design/extensions.md`, `docs/design/m5-extensions-work.md` and the Extensions guide's reference of 2026-11-09 (`books/extensions/src/`). All three predate both today's code and the rewrite, so they are used here as material, not as constraints. It builds on `design.md`, `extensions-first.md` and `extension-api-additions.md` in this folder, and through the last on the read spike's review. NVDA is read at the pinned commit 732d1ce2f.

Every point marked "Decision for Dickson" waits for him. Every other choice is Claude's, marked "(Claude, 2026-11-28)" where it is a choice rather than a consequence. Every figure called an estimate is one, and no Wasm call has been measured in this repository.

Dickson decided on 2026-11-28: Core calls every hook (question 1), and the `Instance` standing step with the instance record as an owner of requests (question 4, the first two points). Every other open question, and the pending `on-node` parity difference, stands on its recommended default, provisional for the prototype, and comes back to him when its feature or third-party extensions arrive, and in every case before the API is frozen at 1.0 (section 23).

This revision takes in the adversarial review (`review-extensions-fable.md` in this folder), cited as "[review M1]" for its must-fix findings and "[review S1]" for its should-fix findings, by number.

## Assessment first

- The model fits the rewrite with little added machinery. An extension is a set of modules compiled to Wasm components and run in a host process of its own. The host is a peer that Core asks, like a reader. Core calls every hook and routes every read, so every decision that changes speech is made in Core, which sees each fact it depends on in order. Rules and declarations are data that Core and the listener apply with no crossing. Every call, read and instance is a request with exactly one ending, so the host's crash, an application's exit, a reload and a cancellation all go down paths the skeleton already has.
- What it adds to `design.md`: the host step kinds `Call` and `Resume` and the `Wants` item, all already approved; one new standing step, `Instance`, whose life is exactly one instance's life, with a `Holding` item; one owner on the closed list of records that own requests, the instance record, which holds and orders every call of its instance, so the host keeps no queue Core cannot see [review M1]; an `Extensions` record; and an instance record per claiming module in each `Application`. The speech-text stage is a step of the speech root's pipeline, not a queue in Core [review M6]. Each of these is listed with its states below.
- Cost, by estimate: an event no extension registered for costs one table lookup in Core and no crossing, so it keeps the 10 ms budget (D15). A handled event costs one crossing to the host and back, plus two crossings for each read. On today's one measured crossing (Core to an outpost and back, 1.0 to 1.5 ms in a debug build) that is about 1 to 1.5 ms per hook and 2 to 3 ms per read, plus the guest's own time. The context sent with each call and reads batched into one request keep most hooks at zero or one read. The first build measures the real figures before any built-in with a hook is ported.
- What is weakest:
  - With no fixed time bound on handlers, something must still give speech back when a speech-text stage never answers. The options are in "Open questions".
  - Core is the one enforcement point and the one router. That is right for ownership, but each read costs two crossings rather than one.
  - One instance runs one call at a time, because the component model does not let a call enter an instance while another is suspended in it. So a holding hook can wait for a running command of the same instance, bounded by the compute bound once Core cancels the command (section 11) [review M1].
  - Two NVDA mechanisms judge a focus before it is queued (`shouldAllowIAccessibleFocusEvent` in Explorer and in Start search), while ADR 0003 commits a focus before its hooks. A veto that needs a live read before the commit has no form yet (question 8) [review S8].
  - The guest target `wasm32-wasip2` makes Rust's standard library import WASI interfaces, so the 2026-11-09 rule "every WASI interface is refused" cannot stand. The host must serve a virtual WASI, and capabilities must be derived from WASI imports too.
  - The surface is large for a first public contract. Claude recommends keeping it at version 0.x, with no promise to third parties, until the inbox built-ins are ported.
- Size, estimated: about 13,500 product lines for the machinery (host, API crate, Core's part, guest SDK, installer and cache, the supervisor's containment), against about 9,500 lines of NVDA's add-on, app module, global plugin and extension-point handlers with its add-on store. That is about 1.4 times NVDA's lines; without the store on either side it is about 3.8 times, because of the sandbox, the process split and the typed contract. The built-ins come to about NVDA's own lines [review S10].

Decisions for Dickson, grouped (each set out in "Open questions" with options and a recommendation):

1. Who calls hooks: Core (recommended), the outpost, or Core with direct reads by the host.
2. The trust model (item 2): one contained process per extension with capabilities derived from imports (recommended), or the alternatives; the lock-screen rule over every read, context, call and effect; the context limited to the module's scope; a capability for commands bound to unmodified keys; and how far Report title and Report focus may read.
3. The API as a public contract (item 1): the context argument, typed reads and text, the sixteen event kinds, following hooks, per-registration end causes and cancelled outcomes, spelling, the clipboard, settings, and the version policy, approved as one set or interface by interface.
4. The protocol (item 4): the `Instance` standing step with its `Holding` item, the instance record as an owner of requests, the clipboard's answer, and the speech root asking stage hosts (also item 3).
5. Long-running handlers: the three behaviours and which hook takes which, how a holding call meets a running command of the same instance, one read outstanding per instance rather than per extension, and what gives speech back from a stage that never answers.
6. Scope (item 5): which of the M13 items the rewrite's extensions include from the start: the speech-text stage, network imports, the data folder and timers.
7. WASI under `wasm32-wasip2` (items 1 and 6): which WASI interfaces the host serves and how each shows in the install dialog.
8. A focus veto that needs a live read before the commit (Start search), against ADR 0003's commit before hooks.
9. Which process registers the extra UI Automation events (item 3).

## Terms

- Extension: what a user installs, one `.vext` file holding an extension manifest, its strings and one or more module folders. NVDA's add-on.
- Module: one contribution inside an extension, of one kind (below).
- Built-in: a module that ships with Verbatim. The built-ins together form one internal extension that the user never sees listed and cannot disable. Each is GPL, written in Rust and compiled to a Wasm component.
- Host: the process that runs one extension's modules. Every host is a peer served by the supervisor.
- Instance: one live copy of a module in its host, with its own memory. An app module has one instance per process of each application it claims; a global module and a speech-text stage have one each.
- Lane: the worker thread in a host that runs one instance. A lane runs one call at a time.
- Rule: a matcher and an action, applied by Verbatim as data with no call into the module.
- Declaration: data in a manifest that changes Verbatim's own behaviour, such as `sleep-mode` or the Calculator's typing-echo override.
- Hook: a function a module exports and Core calls.
- Holding hook: a hook whose answer can change the speech of the event it is called for (`drop`, `replace`, `rewrite`), so the event's default handling waits for it.
- Following hook: a hook whose answer cannot change that speech, so nothing waits for it.
- Context: the record Core sends with every call, holding the facts Core already has. A hook that needs only the context makes no read.
- Crossing: one message from one process to another and its answer, two wakes.
- Capability: a group of imports. A component's capabilities are read from its import list, never declared.
- Layer: one of the three sources of behaviour for an application, highest first: a developer-folder module or the one installed extension that claims it, the built-in, and Core.

The protocol's own terms (request, ticket, held view, generation, stage, end cause, record-owned request) are `design.md`'s.

## 1. What extensions must do

From what NVDA's add-ons and app modules do, read in the pinned source:

- Change how an application is read. NVDA runs each event through the global plugins, then the focus object's app module, then the tree interceptor, then the object, each able to stop the chain by not calling `nextHandler` (`eventHandler.py` 140 to 165). Its 78 app modules (`appModules/`) use `chooseNVDAObjectOverlayClasses`, `event_NVDAObject_init`, `event_*` handlers, scripts, `isGoodUIAWindow`, `isBadUIAWindow`, `shouldProcessUIANotificationEvent`, `sleepMode` and `statusBar` (`appModuleHandler.py` 461 onward). Counted across the 78 (`docs/design/extensions.md`, appendix A): class choice in about 50, mostly one property overridden or a focus vetoed by a predicate, which are rules; object-init hooks in about 25, also mostly rules; event handlers in about 30, which are hooks; scripts in about 20.
- Add commands anywhere. `scriptHandler._yieldObjectsForFindScript` yields the global plugins before the focus's app module and before `globalCommands`, so an add-on can bind any gesture ahead of NVDA's own (`scriptHandler.py` 161 onward).
- Change what is spoken. NVDA applies `speech.extensions.filter_speechSequence` inside `speech.speak`, before the sequence reaches the speech manager (`speech/speech.py` 1137), and announces the result through `pre_speech` (1148). NVDA itself registers its language detection there (`speech/__init__.py` 172). A handler runs synchronously in the caller of `speech.speak`, so a translation that waits on a network service delays that speech for as long as it waits.
- Speak, spell, copy, show messages and lists, click, play tones.
- Keep state per application process, which NVDA keeps in the `AppModule` instance, one per process (`appModuleHandler.fetchAppModule`, 291).
- Have settings and data, and ship strings in many languages (`addonHandler`, manifest specification at 1098).
- Drive output devices: synthesizers now, braille displays in M18.

From the rulings (Dickson, 2026-11-28 unless dated):

- Anything an NVDA add-on could implement through Verbatim's extension API is an extension. What add-ons build on stays in Core.
- The hot path stays in Core: key intake, caret and typing echo, event intake and the queue, event speech, and the speech pipeline. The review cursor, object navigation and say-all commands stay in Core.
- Extensions may change speech, so an event an extension handles may exceed the 10 ms budget by the extension's measured cost. Events no extension handles keep the budget.
- No fixed time bound on handlers: a translation may take seconds.
- Built-ins are GPL, Rust, `wasm32-wasip2` components run by Wasmtime with the component model (Wasmtime decided 2026-10-22). The SDK and the API crate are original code.
- Module settings, on the settings dialog and in the settings file, and reading Verbatim's settings: yes.
- Alt held as a matchable property: yes.
- Commands that pass their key through and do not hold the queue: yes.
- Extensions are replayable in the simulator and not compared with NVDA, except built-ins that port NVDA behaviour, which are compared with NVDA's own app modules and `globalCommands` in the headless oracle.

## 2. The line between Core and extensions

The test (from `extensions-first.md`, section 1, with Dickson's rulings): a behaviour is an extension when NVDA implements it in an app module, or an add-on implements it, or it is a command that needs only the extension API's reads and effects. It stays in Core when it is on the hot path, when NVDA implements it in `eventHandler`, `api`, `NVDAObjects`, the API handlers, `review`, `speech`, `sayAll`, `braille` or the keyboard handler, or when it owns state that other features read. Reason: NVDA's own placement settles the doubtful cases, and what add-ons build on must be one owner's.

The hot path is the five steps `extensions-first.md` defines ("The hot path, defined"). An extension changes it only by data: rules and declarations that Core and the listener apply as table lookups. A hook on a hot-path event is allowed where it is registered narrowly, and its cost is measured and charged to the events it handles (ruling above).

The synthesizer is an output driver on step 5 and stays an extension kind (Dickson, 2026-11-09). Its host's asker is the speech root, not Core (`design.md`, M6).

The built-in modules are listed in full in section 20.

## 3. Kinds of module

| Kind | What it is | Instances | Asker | When |
|---|---|---|---|---|
| App module | Changes how one or more applications are read and adds commands while the focus is in them. NVDA's `AppModule`. | One per process of a claimed application | Core | The rewrite's extensions build |
| Global module | Adds commands that work everywhere, and may read the applications its manifest names. NVDA's `GlobalPlugin`. | One | Core | The same |
| Synthesizer | Speaks: receives speech sequences, returns audio and marks. A native module behind the C interface `verbatim_synth_v1` (Dickson, 2026-11-09). eSpeak NG and OneCore are built-ins, each in a host of its own. | One per voice in use | The speech root | M6 |
| Speech-text stage | Takes each utterance's text and language tags and returns new ones, for translation, language detection and rewriting. NVDA's `filter_speechSequence`. | One | The speech root (section 9) [review M6] | M13 by the roadmap; Decision for Dickson whether from the start (question 6) |
| Recogniser | Receives an image Verbatim captured and returns text, word boxes or a description (roadmap M13, with M8.5's OCR). | One | Core | M13 |
| Braille driver | Drives a braille display through devices Core opens for it (roadmap M18). Display gestures are items of a standing request Core opens on the driver's host (`design.md`, M18). | One per display | Core | M18 |

Scripted modules and the interpreter console are M10's: an interpreter compiled to a component, evaluating against the same API (roadmap, M10). One observation for that milestone: componentize-py, a Bytecode Alliance tool, compiles Python into a component, which may matter to NVDA add-on authors. It has not been tried here.

Reason for one mechanism across kinds: every kind gets the same host, containment, capability derivation, breaker and replay. A kind differs only in its interface and its asker.

## 4. Packaging and manifests

These carry over from the 2026-11-09 reference with the changes named. The reference pages (`books/extensions/src/reference/manifests.md`, `matchers.md`, `rules.md`, `commands.md`) are the material.

- An extension is a `.vext` zip that its author makes by hand: `manifest.toml` at its root (`id`, `name`, `description`, `author`, `version`, `api`, `modules`), `locale/<language>/*.ftl`, and module folders. There is no packaging tool. Reason: NVDA's add-ons are zips too, and a tool would be one more thing to maintain (Dickson, 2026-11-09).
- A module is a folder with `module.toml` and, unless it is made only of rules and declarations, a component. Its keys: `kind`, `component`, `applications` (an app module's application identities, every name its application has gone by), `reads` (a global module's readable applications), `sleep-mode`, `[[rules]]`, `[[hooks]]`, `[[commands]]`, and, new here, `[[settings]]`, `reads-settings` and the declarations such as `[typing-echo]` (section 13).
- A built-in is the same folder under the program's `extensions/builtin/` (planned), with its strings in Verbatim's own Fluent resources and no extension manifest.
- Install extracts through a hidden staging folder beside the destination, renamed into place only once every file is written and checked (security model, X13). Reason: a package that writes outside its folder runs code with the user's rights and undoes the sandbox.
- Anything unrecognised in a manifest is refused with the key named, never ignored. The checks run again at every load, so a folder changed after install is marked failed rather than loaded. Reason: parse at the boundary; a misspelt key is an error the author sees at once.
- At most one installed extension claims an application, above the built-in, and no two modules of one extension claim the same one; a second is refused at install (Dickson, 2026-11-09). Reason: installed extensions then need no order among themselves.
- The developer folder works as one extension, in one host of its own; its module replaces an installed extension's module for the application it claims while it is there, as NVDA's scratchpad does (Dickson, 2026-11-09).
- Application identity has one resolver: the listener resolves it once per process and sends `Process { pid, identity }` before the first item naming that process (`design.md`, "Items"). The identity table (executable; a console's client; `wwahost.exe`'s hosted application; `msedgewebview2.exe`'s parent) is Core's data.
- Matchers keep the reference's grammar (exact, lists, `all`, `any`, `not`, `prefix`, `regex`, `at-least`, `below`), with one type and evaluator in the model crate and its parser in the API crate. New property: `alt-held`, true when Alt was down as the event arrived, read from the stamp Core already keeps (`design.md`, async rule 8; Dickson, 2026-11-28). Reason: the Alt+Tab switcher's container suppression is then a rule with no hook.

## 5. The trust model and the sandbox

Trust boundaries are Dickson's (item 2). The options:

| Option | What contains an extension | Cost | What it gives up |
|---|---|---|---|
| A. Wasm sandbox plus one contained process per extension (the 2026-11-09 model) | Wasm allows only the imports a component was built with; each extension's host runs in an AppContainer with a memory cap and a processor rate cap; built-ins share one host; the developer folder has one host | One process per installed extension, so memory and start-up time per extension (to be measured) | Nothing; native code is contained by the process |
| B. Wasm sandbox only, every extension in one shared host | Wasm alone | One process for all extensions | A crash or hang in one extension costs all of them; native code cannot be contained and must be refused for installed extensions |
| C. Option A with permissions the manifest declares and the user grants at run time | As A, plus a prompt on first use | A prompt the user must answer while working | Nothing more is contained, since imports already bound what a component can call; it only adds prompts |

Claude recommends A. Reason: the guide's three goals (recovery, containment, honest consent; `books/extensions/src/explanation/security-model.md`) need the process boundary for native code and for a hang; imports already state everything a component can call, so a declared permission adds nothing a reader can trust more.

Under A, the rules (each carried over unless marked new):

- Capabilities are derived from the component's imports, never declared. The install and Extensions dialogs are built from them (layout and wording: `books/extensions/src/reference/capabilities.md`). Reason: the import list is written by the compiler and is a ceiling the code cannot exceed.
- The manifest carries only what an import cannot say: network hosts, devices a braille driver drives, and, for native code, whether it uses the network.
- New: Core enforces the application boundary, since every read goes through Core. Core parses each requested read into a checked batch against the instance's applications and refuses, without sending, any step whose subject lies outside them. A navigation that leaves them answers "not there" for that node. Reason: Core is the one process that sees every read and knows which instance asked; under the 2026-11-09 design the outpost judged by the pipe, which no longer exists.
- Ids given to a guest are resources the guest cannot forge. The host maps each resource to an id, and Core refuses any id the instance's record does not name. Reason: an id is parsed once at the boundary into something only Core issued.
- An update that adds a way for content to leave (a network host, a device), or a new source of the user's content in an extension that can already send data out, is installed only after the user approves it again.
- Native code, either a library called through a narrow import or a whole native module for a kind with a small C interface, runs in its extension's host, contained by the process. The install dialog warns "Contains native code that runs outside Verbatim's extension sandbox".
- On secure screens (M8.7) only a machine-wide allowlisted copy loads, with a scratch folder in place of the data folder (Dickson, 2026-11-06).
- Recovery: Verbatim can start with every extension off (A8); the breaker turns off an extension that keeps failing (section 6).
- New capability rows: "moves the caret in it" for the text interface's `select`; a warning "Can send what you copy to <host>" for clipboard reading combined with a network import (`extension-api-additions.md`, section 4). Decision for Dickson, with the API (question 3).
- The context stays inside the module's scope, as reads do [review M7]. Core fills each field of the context only with what lies within the instance's applications (an app module's claimed applications, a global module's `reads`): a focus, an ancestor or a foreground in another application is given as `outside`, with no values. A module that needs the focus or foreground anywhere declares it in its manifest (`reads = ["focus", "foreground"]`, below), and the install dialog shows it as "Reads the focused control in every application". Reason: under the 2026-11-09 design the context did not exist; as first drafted here it carried other applications' content past the boundary Core enforces on reads.
- Commands bound to unmodified keys [review M7]. A command whose gesture has no modifier other than Shift (a character key, a digit, a function key alone) sees the user's typing wherever it applies. In an app module it shows "Sees what you type in <application>" on that application's row; in a global module it shows the warning "Sees what you type in every application", and with a network import, "Can send what you type to <host>". Reason: typed characters are kept out of hooks for the hot path, and a pass-through binding over every key would bring them back unseen by the user.
- The lock screen. NVDA's Report title refuses when the foreground is below the lock screen while Windows is locked (`globalCommands.py` 3210 to 3230, `objectBelowLockScreenAndWindowsIsLocked`). Recommended (question 2): while Windows is locked, nothing below the lock screen reaches an extension, by one rule Core applies at every send and arrival [review M7]: every context field below the lock screen is `outside`; every read whose subject is below it, a handle held from before the lock included, is answered `outside` without sending; no event from a window below it is delivered to a hook; and a running call's `speak`, `speak-node` and list dialog for an application below it are dropped on arrival. Core judges "below the lock screen" from `Foreground`'s lock state and input desktop, which it owns. The other option gives the node with a `below-lock-screen` property and trusts each module to check it.
- Reach of global modules [review S6]. Report title and Report focus need the foreground and the focus in any application, and the tray lists need Explorer. ADR 0003 leaves "a built-in module does not read across its application's boundary" to this design, and the roadmap puts "global modules reading every application" in M13. Recommended (question 2): the narrow scope `reads = ["focus", "foreground"]`, which reaches only the node Core holds as the focus or the foreground and its ancestors, for built-ins and installed extensions alike, shown as above; reading every application stays M13's.

Limits: Verbatim does not keep content from an extension the user installed, and does not defend the install dialog against other programs the user runs (the guide's stated non-goals). An extension that answers in time but wrongly is not caught; starting with extensions off recovers.

## 6. Hosts as peers

### Processes

- Every host is served by the supervisor through `Serve(Role::ExtensionHost(id))` (`design.md`, "What each ask means"). Its items are `Started`, `Ready` and `Pong`; its final outcome is `Ended(reason)`. Its life is that request's life, and its generation is that request's number.
- The built-in host is started with Verbatim and always runs, since global modules and the first focus need it (Dickson, 2026-11-09). An installed extension's host is started when the extension is first needed: an instance due, or a compile at load. It is ended when no instance remains open and no compile is running (Claude, 2026-11-28, from security model X4).
- Host and lane are the reader shape (`design.md`, "Reader reducers and workers"): the host's root reducer holds a `Lane` per open instance. Each lane is a worker whose arena holds the Wasm store, so no store crosses threads.
- Core runs no module code and links no Wasm runtime; neither do the listener and the outposts. Only the host program and the simulator link Wasmtime.

### The messages

All are the existing protocol's (`design.md`, M5), with one new step:

- `Instance { module, version, application }` (new, a standing step): opens a lane with a fresh instance. One `Instance` request is exactly one instance: the host never re-creates an instance under the same request, and any new instance, after a trap, a reload or an abandoned lane, is a new request Core opens [review M4]. Its one kind of item is `Holding(ids)`: the whole set of ids the instance's guest holds handles for, sent by the host at the end of every call, whatever that call's ending, a cancelled call's included [review M5]. Its `Final` is `Ended(Trapped)`, `Ended(OverCompute)`, `Ended(OverMemory)`, `Ended(Stuck)` or `Ended(Cancelled)`. Reason: an instance then has one ending, and the host's crash, a reload, a disable and the application's exit all reach it through its ticket. Decision for Dickson (item 4, question 4). The other option opens a lane implicitly at its first `Call` and closes it with a separate step, which leaves the instance's life with no request, so its end needs a path of its own.
- `Call { instance, hook, context, args }` (approved): a standing step naming its `Instance` request. Its items are `Effect` (speech, tones, a list dialog, a click, a settings change, a log line) and `Wants(batch)`; its `Final` is the verdict. Core sends a call only when the instance runs none (below), and the host answers every call before its instance's `Final`.
- `Resume { call, results }` (approved): a one-shot step answering a `Wants` with the read's outcome.
- The host writes every item of one guest run, its effects, its `Wants` or verdict, and the `Holding` item after a verdict, in one pipe write, and Core takes every message waiting when it wakes, so they add no wake (Claude, 2026-11-28).

### Records in Core

- `Peers.hosts: BTreeMap<HostId, Host>`, each with the same serving life as an application (`design.md`, Core's records). The host generation's `Out` holds every request out to it, each with its owner's address.
- `Extensions`: one state value per extension (section 7).
- An instance record for each module that claims the application, at most one per layer, in that application's `Application` record. A global module's instance record is in its extension's `Active` state. Reason: state about an owner lives in that owner's record, so the process's exit drops its instances by construction, and so does an extension's disable.
- The instance record owns and orders every call of its instance, so the host keeps no queue Core cannot see [review M1]. A holding call's ticket is its entry's, and the record holds its address; a non-holding call's ticket is the record's own. Either way the record lists every call due or running, so the instance's end resolves each of them in the same transition [review M4]. This makes the instance record an owner on the closed list of `design.md`'s rule 1 (question 4).
- The record is one state value:
  - `Closed`: no instance and no call due.
  - `Pending { due }`: calls due while the extension's code is still compiling (`RulesReady`) [review S1].
  - `Open { ticket, version, held, lane, due, dialog }`: `ticket` is the `Instance` request's half; `held` names the ids the instance holds (`Named<Id>`, so the held view to each outpost keeps them); `lane` is `Free` or `Running { call }`, where `call` is `Live(address)` or `Cancelled`; `due` is the calls not yet sent; `dialog` is the list-dialog state, `None`, `Held` or `Open`, carried over from `docs/design/extensions.md` section 4's table with its owner moved here.
  - `Draining { old, held, running, due, dialog, next_version }`: a reload is owed while the old instance's call finishes.
- The order rule [review M1]: due calls are sent one at a time in the order Core made them due, except that a holding call goes ahead of every non-holding call still due. When a holding call becomes due while a non-holding call runs, Core cancels the running call. Cancellation is cooperative (section 11): the guest gets `cancelled` from its next import and returns, so a holding call waits at most for the running call's computation up to its next import, which the compute bound limits, and never for a read or a network wait. The cancelled call's later speech is lost; that is the rule's cost.
  - Why not a lane of its own for holding hooks: a holding hook and a command share the module's state (Calculator's flag is set by a command and read by a notification hook), and a second instance has none of the first's memory.
  - Why not let the holding call wait: with no bound on handlers, it would hold the event queue for as long as the command waits.
  - Why the host cannot run them together: the component model does not let a call enter an instance while another call is suspended in it.

| Rule | `Closed` | `Pending` | `Open` | `Draining` |
|---|---|---|---|---|
| A call is due and the code is ready | `Instance` and the call sent in one write; to `Open` with the lane `Running` | none can be: the code is pending | lane `Free`: sent; lane `Running`: placed in `due` by the order rule, and a holding call due behind a non-holding running call cancels it | placed in `due` by the order rule |
| A call is due and the code is pending | to `Pending` with it | added to `due` | none can be | none can be |
| The code result: ready | nothing | `Instance` and the first due call sent; to `Open` | nothing | nothing |
| The code result: failed, or the registration removed | nothing | every due call resolved `Ended`; to `Closed` | nothing | nothing |
| An `Effect` item | none can arrive | none can arrive | judged as section 9 says | judged |
| A `Wants` item | none can arrive | none can arrive | parsed into a checked batch against the instance's scope; a refused step answered without sending; otherwise sent to the outpost as the call owner's request (stage `ext.read`); the ids its answer returns are added to `held` | the same |
| A call's `Final` | none can arrive | none can arrive | a `Live` call's verdict to its owner, a `Cancelled` call's dropped as late; lane `Free`; the next due call sent | as in `Open`, then the old instance cancelled, `Instance` of `next_version` sent with the first due call, to `Open` |
| A `Holding(ids)` item | none can arrive | none can arrive | `held` becomes those ids, less any Core never gave this instance | the same |
| The call's owner cancels it (an end cause, the entry's end, the utterance's cut) | none is due | removed from `due`, resolved | due: removed unsent, resolved; running: `Cancel` sent, ticket resolved at once, the lane stays `Running` with the call `Cancelled` until its `Final` | the same |
| The instance's `Final` | none can arrive | none can arrive | the running call and every due call resolved `Ended` in this transition; `held` dropped, so the next held view releases those ids; `dialog` closed with no answer; the breaker counts it unless `Cancelled`; to `Closed` | as in `Open`, then to `Open` with a new `Instance` if `due` is not empty, else `Closed` |
| The host's end | nothing | nothing | as the instance's `Final`, the host generation's `Out` handing back every ticket | the same |
| The application's process exits | the record is dropped with `Application` | due calls resolved `Ended`; dropped | `Cancel` of the instance and of the running call in the same step; every call resolved; dropped | the same |
| The extension leaves `Active` (disable, the breaker) | nothing | due calls resolved `Ended`; to `Closed` | `Cancel` of the instance; every call resolved; to `Closed`; for an installed extension the host is ended, which does the same for all its instances | the same |
| A reload is published | nothing | stays, waiting on the new version's code | lane `Free`: `Cancel` of the old instance, and the next due call opens the new; lane `Running`: the running call cancelled, to `Draining` | `next_version` replaced |
| The outpost's generation ends | nothing | nothing | `held`'s ids of that generation stop being named; any read naming one answered "not there" without sending; the instance stays open | the same |

A front entry waiting on a `Resume` when its instance ends has its call's ticket resolved `Ended`, and goes on with its registration's cancelled outcome (section 11) [review M4].

Lifecycle states this adds to Core: the instance record's four variants, the lane's two, the call's two, the dialog's three (carried over), and the breaker's failure times per module or extension in `Active` [review S7]. Removed against the 2026-11-09 design: the outpost's hook state (`NoHook`, `AwaitingHost`, `AwaitingHook`), the host pipe's `Waiting` and `Ready`, the liveness alarm `stalled_at`, the host-side deadline clock, and the host's own call queue.

### The host's root

- `HostRoot { serve, lanes: BTreeMap<RequestNumber, Lane> }`, one lane per open `Instance` request.
- `Lane { module, version, worker: WorkerSlot, running: Running }`, where `Running` is `Idle`, `Computing { call }`, `Suspended { call, wants }` or `Importing { call }`.
- The lane runs at most one call, since Core sends no other until its `Final` [review M1].
- Every host import that can block, file input and output, name resolution, the network, the clipboard's answer, runs as an asynchronous host function on the host's own input and output threads, so a lane waiting on it is `Importing` and reaches its cancellation point at once; the blocked operation finishes on its own thread and its result is dropped [review S3]. `wasmtime-wasi` runs its blocking work on Tokio's blocking pool, which is read from its source before this is relied on. The one synchronous import is the narrow import into a native library.
- A lane that has not answered a cancelled call within the grace is abandoned: its worker is left, the instance's request ends `Ended(Stuck)`, and the lane is removed. Core opens a new `Instance` at the next due call [review M4]. Only a native call can hold a lane past the grace, since epoch interruption reaches all Wasm code and every other blocking import is asynchronous. The grace is Claude's proposal, 1 second, measured before it is fixed; it bounds the time after a cancellation, not a handler's own time.

### Crashes and the breaker

- A host's end, by crash, by its memory cap or by the supervisor, resolves every ticket on that generation once. A holding hook's entry goes on with its cancelled outcome; a non-holding call is dropped; every instance record goes to `Closed`. The supervisor restarts the built-in host with the outposts' backoff. An installed extension's host is started again at its next due call.
- Each instance has its own memory limit in its store (Wasmtime's store limits), well below the host's cap, so a module that grows past it ends `Ended(OverMemory)`, which names it, rather than ending the host [review S5].
- The breaker: three failures to respond within one minute turn off an installed extension for the session, or one built-in module, which then falls back to Core's behaviour, spoken once (Dickson, 2026-11-09, security model A9). A failure to respond is an instance ending `Trapped`, `OverCompute`, `OverMemory` or `Stuck`, or the host's end with a call of an installed extension running. An answer that reports a failure, such as "Translation failed", is not counted, and neither is time spent waiting.
- The built-in host's end names no built-in, since a process-wide fault names none. So three ends of the built-in host within one minute with no module named turn off every built-in module for the session, spoken once, "Built-in modules were turned off after repeated failures", as A1 runs Core without extensions after repeated crashes (Claude, 2026-11-28) [review S5].

### Held views and generations

- The host is not a reader of application objects: it holds no COM object, and the ids it holds are the outposts'. So the held-view rule applies between Core and each outpost unchanged, computed from Core's records, the instance records among them.
- What an instance holds is the host's fact, reported whole in each `Holding` item; Core's `held` takes each report and judges nothing from it but the held view [review M5]. Ids a read returns are added to `held` when the read answers, so they are kept until the next report. A cancelled call's handles go at the `Holding` item that follows its end, and every handle goes at the instance's end. A stale `held` only keeps more, never less.
- The 2026-11-09 design's "generation" of rule tables becomes two things: the listener part rides in the `Watch` filters, so its generation is the watch number (`design.md`, "What each ask means"), and everything else is Core's own data, which needs no publication. The gesture map is published to the hook thread at a version, as today.

## 7. Loading, the lifecycle and reload

- A loader thread of Core's reads folders and `.vext` files, parses and validates manifests and rules in the API crate, derives capabilities from each component's imports, and has the extension's host compile and pre-link the components into the content-named cache. It reports two results per extension, each final: its rules, then its code. A host that ends during the compile makes the code result "failed: host ended" [review S1]. Conflicts are decided from the whole set of manifests a scan found, never by arrival order.
- The extension's state in `Extensions`, one value (from `docs/design/extensions.md` section 2.2):

| State | Data | Left by |
|---|---|---|
| `Loading` | none | its rules result, to `RulesReady` or `Failed` |
| `RulesReady` | version, rules, declarations, registrations and gesture bindings, the code pending | its code result, to `Active` or `Failed` |
| `Active` | version, rules, declarations, registrations, gesture bindings, global instance records, the breaker's failure times; for the built-ins, the modules the breaker turned off; a reload's first result while its second is awaited | a reload, which publishes both new results together once both arrive or keeps this version if either fails; disable or the breaker, to `Disabled` |
| `Failed` | reason | a reload in developer mode, to `Loading` |
| `Disabled` | by the user or by the breaker | enable, to `Loading` |

- The rules come first because compiling the built-ins on a cold cache is slow. The 2026-11-09 design says it takes seconds, which has not been measured. Core takes no input until the built-in extension has left `Loading`, so the first focus is judged with the built-ins' rules.
- A call due while its code is pending waits in its instance record's `Pending` variant, as any due call waits, with its owner's end causes still applying: a holding hook's entry ends on its declared causes, and a command's call runs when the code is ready, its speech judged then by the key fence [review S1]. Gesture bindings are published with the rules, so Verbatim+T and Verbatim+Tab are bound during the compile and their presses run once the code is ready. A failed compile resolves every waiting call `Ended`: a holding hook takes its cancelled outcome and a command says nothing, which is logged.
- Reload in developer mode: the folder watch, debounce, full validation, and the instance records' reload rows above, so a call already running finishes or is cancelled and the next one runs on the new version, its state lost as in NVDA's reload. Speech: "Loaded <module>", "Reloaded <module>", "Reload of <module> failed" with the reason's first line (Dickson, 2026-11-09).
- A failed built-in is spoken at once: "Built-in module <name> failed to load".

## 8. How events reach extensions

### Registration and matching

- A hook is registered in `module.toml` with its kinds, its backend, a matcher over the event's control, and, for a holding hook, its end causes and its cancelled outcome (section 11). Core calls nothing for an event no module handles.
- Core matches every registration (Claude, 2026-11-28). It keeps an index keyed by application identity and event kind; a tell finds its entries with one lookup, and with none the event has no extension stage. Reason: one owner judges every hook registration, so no reader judges from rule data on Core's behalf. The listener applies only the rules that drop or veto events, to save crossings (ADR 0003 allows drops on an event's own cached properties).
- A hook's matcher names only what the event already carries, so matching makes no call [review M8]. The load check refuses any other property. What an event carries:
  - for every kind, the properties of the listener's cache request for that kind, which Core's `Watch` filters set: each property a registration names for a kind joins that kind's cache request while the registration is active, so UI Automation fetches it with the event rather than by a later call. The cost is a larger event, measured before the first built-in with a hook ships; it falls on every event of that kind while the registration is active, and the ledger names the registration that added it;
  - for `focus`, also Core's own focus read with its ancestry, so an `on-focus` matcher may name an ancestor's cached properties;
  - for `notification`, its kind and activity id.
- A property that cannot be cached with an event, such as a relation (a parent's automation id) or a live state, cannot be matched; the hook reads it itself, one read charged to the events its matcher admits.
- Rules: window rules (`backend`, `silent`, `normalize-class`) and event drops and focus vetoes on cached properties ride in the `Watch` filters; node rules (`set`, `clear`, `transform`, states, `presentation`, `position`, `drop`, `veto-focus`, `redirect-focus`) and `accept` apply in Core to answers. Within one event, every layer's rules apply first, lowest layer first, so the highest layer wins a contested property; then holding hooks run from the highest layer down, each seeing the node as the layers above left it (Dickson, 2026-10-22 and 2026-10-28). `pass` and `rewrite` continue the chain, `drop` and `replace` end it, and a drop is never undone by a lower layer.
- Provenance: for every property a rule or hook changed, Core records which module and rule set it, shown in the focus report. It is data of each announcement, not a lifecycle.

### The event kinds

The 2026-11-09 reference's five, and eleven more drawn from NVDA's inbox app modules (`extension-api-additions.md`, section 6, which counted every `def event_` in `appModules/`):

| Kind | NVDA's event | Data besides the control |
|---|---|---|
| `focus` | `gainFocus` | none (the ancestors are in the context) |
| `selection` | `UIA_elementSelected`, `selection` | none |
| `name-change` | `nameChange` | the new name, or none |
| `value-change` | `valueChange` | the new value, or none |
| `notification` | `UIA_notification` | kind, processing, text or none, activity id or none |
| `window-opened` | `UIA_window_windowOpen` | none |
| `shown` | `show` | none |
| `state-change` | `stateChange` | the states before and after, as Core holds them |
| `item-status` | `UIA_itemStatus` | the new status, or none |
| `focus-entered` | `focusEntered` | none |
| `application-gained` | `appModule_gainFocus` | none |
| `application-lost` | `appModule_loseFocus` | none |
| `live-region-change` | `liveRegionChange` | none |
| `selection-added` | `selectionAdd` | none |
| `selection-removed` | `selectionRemove` | none |
| `selection-within` | `selectionWithIn` | none |

- `on-node` replaces `event_NVDAObject_init`: called for each batch of matching controls that a front entry of Core's reads for Core's own use, returning changes that apply wherever the control goes. Which reads it applies to [review M2]:
  - It applies to the reads a queue entry makes for Core's own speech: the focus walk, an event's read, the announcement's reads for `speak-node`, and the navigator's and review's reads, which are queue entries.
  - It never applies to a module's `Wants` read, of its own module or another's. Reason: a module's own read would owe a call to the very instance suspended on that read, which could only wait behind itself. A module reads the raw controls and applies its own logic.
  - It never applies to a record-owned read (terminal output, progress, and the rest of `design.md`'s list), since a record's answer is delivered in its step and has nowhere to wait. Node rules still apply to both, with no call.
  - Limit: NVDA applies `event_NVDAObject_init` to every object, an app module's own included (Calculator builds child objects inside its handler, `calculator.py` 52 to 59). So a third-party module reading Calculator's history items sees the names the application gives, not the built-in's. The built-in's own effect on Verbatim's speech is unchanged, since Core's reads carry it. Recorded in the parity ledger as pending.
- Not offered as hooks, because they are the hot path: typed characters and caret movement. Calculator's typing-echo override is a declaration (section 13). `foreground` is the window rules and `application-gained`.
- `application-gained` and `application-lost` run before the focus they come with is committed; every other focus hook runs after the commit, as NVDA does (ADR 0003, "Extensions").
- A kind Verbatim does not otherwise listen for (`window-opened`, `item-status`, `live-region-change`) needs a UI Automation registration in the listener, made while any active module registers the kind. Whether it is desktop-wide with a process filter or per window of the claimed application is a thread and process question, Dickson's (item 3, question 9).
- A new kind whose data needs a new WIT type is part of the contract (question 3).

### Cost when unhandled

One index lookup in Core per tell, with no crossing and no wake (estimate). Two costs fall on unhandled events too, and only while a module registers them: a property a registration adds to a kind's cache request enlarges every event of that kind, and a kind that needs its own registration adds the listener's handling of its events. The ledger check `extensions-first.md` proposed stays and is widened: a stage that awaits a host is named in the stage list; a hot-path entry kind awaiting a host stage fails the build unless its registration is listed with its measurement; and every property a registration adds to a cache request is listed with the registration and the measured size of the events it enlarges (Claude, 2026-11-28) [review M8].

## 9. Hooks that change speech

The hooks:

| Hook | Called | Answer | Kind of hook | Modules |
|---|---|---|---|---|
| `on-focus(ctx)` | a committed focus matching the registration | `pass`, `drop`, `replace(node)`, `rewrite(changes)` | holding | app |
| `on-event(ctx, event)` | an event of a registered kind and matcher | `pass`, `drop`, `rewrite(changes)` | holding, or following with `follow = true` | app |
| `on-node(ctx, nodes)` | each batch of matching controls a queue entry reads for Core | the changes for each | holding | app |
| `after-focus(ctx)` | once Core has queued its own announcement of a matching focus | nothing | following | app |
| `after-event(ctx, event)` | the same, for an event Core handles itself | nothing | following | app |
| `status-bar(ctx)` | when the user asks for the status bar | a node, or none | holding the command | app |
| `on-command(ctx, command, trigger)` | one of the module's gestures | nothing | section 10 | app, global |
| `on-list-dialog(ctx, dialog, answer)` | the user answers the module's list dialog | nothing | following | app, global |
| `transform(ctx, utterance)` | each utterance the stage takes | the new utterance, or its failure indication | holding that utterance in the speech root | speech-text stage |

Rules for speech:

- Every effect is judged by Core when it arrives, as Core's own speech is: by the key fence (speech of a press a later key superseded is dropped), by attention (speech for an application not in front is queued, never interrupting), by the lock-screen rule (section 5), and by the announcement it belongs to. A stale context therefore costs a wasted read or a dropped utterance, never wrong speech. Reason: Core sees every fact in order; the guest saw a snapshot.
- A holding hook that speaks keeps its place before the default, as NVDA's handler that speaks and then calls `nextHandler`.
- Speech after the default is a following registration, not a verdict (`extension-api-additions.md`, section 5). Core speaks its announcement and sends the call as that announcement is queued, and the hook's speech joins the end of that announcement's speech. It shares that announcement's fate: whatever cuts the announcement (the key fence, a newer focus, cancel speech) drops it. Reason: Core never holds its default for speech that cannot change it. The other option, a verdict such as `pass-then(speech)`, makes every such hook a holding one.
- `speak-node(node)` announces a node as Core announces a focus, styled by the user's theme; Core makes the reads the announcement needs (section 13).
- `replace(node)` commits the named node as the focus and announces it, for redirects in code; it must be a node the instance holds.
- `rewrite(changes)` changes properties for this announcement only; `on-node`'s changes apply wherever the control goes, as a node rule's do.

### The speech-text stage

The stage is a step of the speech root's pipeline, not a queue in Core [review M6]:

- Core composes an utterance and emits `Say` as it does with no stage. The speech root, which owns speech order, interruption and cancellation, sends each utterance a stage takes to the stage's host and passes the answer on to the synthesizer. The speech root is then a second asker of extension hosts, as it already is of the synthesizer's host (`design.md`, M6, tentatively ruled); which process asks is Dickson's (item 3, question 4). Reason: interruption and cancellation are the speech root's facts, so judging a stage's pending utterances in Core would judge them in two places.
- A stage's registration names the utterance kinds it takes (announcements, messages, text read, and so on), so a translator can leave typed characters and spelling alone (Claude, 2026-11-28).
- An utterance of a kind the stage does not take bypasses it and is not held behind it. Limit: an untaken utterance, such as a toast or terminal output, can then be spoken before an earlier taken one that is still being transformed. NVDA keeps the order only because its filter blocks every speech call.
- Never sent through a stage, whatever it takes: Verbatim's own interface speech (its dialogs, menus, settings and the Extensions dialog), its messages about extensions (failures and the breaker), and a stage's own failure indication. Core marks each utterance's source as it composes it, and the speech root routes by the mark. Reason: the speech the user needs to turn a stage off never depends on that stage.
- Taken utterances keep their order among themselves: one waiting for its stage holds the taken utterances after it, and nothing else. It holds no key and no event.
- A pending transform shares its utterance's fate in the speech root: whatever cuts the utterance (the key fence, cancel speech, a newer utterance that interrupts) cancels the call. So typing interrupts a slow translation as it interrupts speech, and the typed echo, which the stage does not take, is spoken at once.
- Each transform is a request owned by the speech root's record of that utterance, with one ending: the stage's answer, the cut, the host's end, or the instance's end, the last two giving the failure indication.
- The original text is never spoken; a stage that fails gives the failure indication it declares, such as "Translation failed" (roadmap, M13, Dickson, 2026-11-02).
- What gives speech back from a stage that never answers is open (question 5).

## 10. Commands and gestures

- A module declares commands in `module.toml` with default gestures, a number of presses, a description and an optional `pass-through`. Every press fires at once; the hook thread alone judges from its published map whether the same keys are bound to a higher count, and stamps the press with `hold_until` only then (Dickson, 2026-11-09).
- Order: the user's own gesture map above every layer; then a global module; then the focused application's app module; then Core's commands, as NVDA's `_yieldObjectsForFindScript` checks global plugins before the application's module (Dickson, 2026-11-09). An app module's commands follow Core's committed focus (Dickson, 2026-11-10). Gesture conflicts are listed in the install dialog, not refused.
- A command bound to an unmodified key carries the capability of section 5, shown at install [review M7].
- A press bound by a module in an active scope is resolved before the caret table, so a module that binds a caret key takes its hold away, as NVDA runs the app module's script in place of the caret script.
- A command call is owned by the instance record and does not hold the queue: later keys and events go on, and its speech is judged by the key fence on arrival, stamped with its press's key number. Reason: a command's own work is not the hot path (ruling); a command that holds the queue would put a host on every later key's path. A holding hook of the same instance that becomes due while the command runs cancels it (section 6's order rule).
- A command bound with `pass-through = true` sends its key on unheld and does not hold the queue (Dickson, 2026-11-28). Its call is made due before any later call of that instance, so Calculator's flag is set before the notification that reads it.
- A module's command starts only once every held key ahead of its press has been passed on and read after, as Core's own commands do.
- `status-bar` is called by Core's Report status bar command; the highest layer that returns a node wins.
- A list dialog a command opens is held until the press's multi-press window has closed and every later press of the same keys has answered, so a double press never shows the first dialog (Dickson, 2026-11-09).
- Limit: a command that must act before later keys reach the application, such as one that moves the focus, has no ordering against them when it does not hold. Those actions are M13's, and their design decides it.

## 11. Long-running handlers and cancellation

### The three behaviours

From `extension-api-additions.md` ("What waits while a handler runs long"):

- Hold: the queue's front entry waits for the answer, and every later key and event waits behind it. This is NVDA's behaviour, since its handlers run on its one main thread.
- Hold ended by causes: the entry waits as in Hold, but its stage declares end causes. When one holds, Core cancels the call and goes on with the registration's cancelled outcome. A slow handler then delays only its own event, and only until the user does something else.
- Do not hold: the entry goes on with the default at once, the call runs beside the queue, and its answer can only add speech, judged when it arrives. This is open only to a hook whose answer cannot change the event's speech.

### Each registration declares its causes and its cancelled outcome

Some hooks exist to act exactly when a focus is pending or a later event has arrived, so no one set of causes fits every hook [review M3]. A holding registration therefore declares:

- `ends-on`: any of `later-key`, `pending-focus` and `committed` (`design.md`'s `LaterKey`, `PendingFocus`, `Committed`). Default `["later-key"]`: a key the user presses ends a slow hook, and nothing else does. A focus that arrives meanwhile does not, so a hook can see `pending.focus` and act on it, as NVDA's do.
- `on-cancel`: `pass` (the default announcement goes on) or `drop` (nothing is said for the event). Default `pass`.

The two NVDA cases, worked:

- Explorer's menu-bar exit (`explorer.py` 428 to 441) drops the focus on a top-level menu item when `isPendingEvents("gainFocus")` is true, and its notification area (line 111) returns without speaking when a focus is pending. Both registrations use the defaults. The focus entry reaches `ext.call` with another focus queued behind it; `pending-focus` is not among its causes, so the call runs, the context carries `pending.focus = true`, and the hook answers `drop`. The pending focus is then spoken as NVDA speaks it. Only a key pressed before the hook answers ends it, and then `pass` lets the default run, which the key fence judges as it judges Core's own speech.
- Calculator's `DisplayUpdated` handler (`calculator.py` 94 to 141) exists to replace the notification's display string, which can be truncated on paste (issue 16573, cited at 132 to 135). Its registration declares `ends-on = ["later-key"]` and `on-cancel = "drop"`. When the focus moves to the equals button while the hook waits on its read, nothing ends it, and it speaks the result. When a key ends it, nothing is said for the notification, so the truncated string is never spoken.

Recommended behaviour per hook (question 5):

| Hook | Behaviour | Default causes and outcome |
|---|---|---|
| `on-focus`, holding `on-event`, `on-node` | Hold ended by causes | `ends-on = ["later-key"]`, `on-cancel = "pass"`, each overridable |
| `after-focus`, `after-event`, `on-event` with `follow = true`, `on-list-dialog` | Do not hold; speech shares the fate of what it follows | none |
| `on-command` | Do not hold; speech judged by the key fence | none |
| `status-bar` | Hold ended by causes | `later-key`, and the command says nothing |
| `transform` | Holds only its taken utterances, in the speech root | cancelled with its utterance |

### Cancellation

- Core cancels a call by `Cancel` on the call's standing request: for a declared end cause, the entry's end, the instance's end, the order rule of section 6, or, for a transform, the speech root's cut. The owner's ticket is resolved at once; anything the call sends after that is dropped as late, except the instance's `Holding` item.
- At the host the cancellation is cooperative everywhere [review S2]. A guest suspended in a read, waiting in an import, or computing gets `cancelled` from that import or from its next one, and its code after it runs only to return. Its state is kept, so a key landing while a translator parses a response or while Calculator's hook computes loses nothing. Its computation until then counts against the compute bound. Reason: no forced re-creation, so state is never lost by timing, and replay need not model epoch ticks for a cancellation; Core's ticket is already resolved, so letting the guest finish costs only the lane.
- A lane that reaches no cancellation point within the grace is abandoned (section 6).

### Bounds

- No bound on a handler's wall-clock time (ruling). Time spent waiting on reads and on the network is not bounded by Verbatim.
- A compute bound on the guest's own processor time per call, counted in epoch ticks while the guest computes and paused while it waits in an import, measured from the built-ins and set generously, as A6 and the 2026-11-09 design set it. Past it the instance ends `Ended(OverCompute)`. Claude reads the ruling as about waiting, since a translation's seconds are network time, and a guest spinning on the processor is a defect. Whether a compute bound counts as a time bound under the ruling is a Decision for Dickson (question 5).
- One read outstanding per instance, which the lane's suspension gives with no state. ADR 0003 rules one per extension; keeping that needs a waiting list of reads in the extension's record, and its reason (an extension cannot crowd Core's reads) is met per instance too. Decision for Dickson (question 5).
- ADR 0003's "Core's own requests to an outpost are served first" meets `design.md`'s first-in, first-out pipes with no priority. Under this design the bound is one extension read per instance ahead of a Core read; the request class on `Ask` that `design.md` keeps in reserve is the fix if a desktop run shows Core's reads delayed (Claude, 2026-11-28).

## 12. Reads and text

The interfaces are `extension-api-additions.md`'s sections 1 to 3, summarised here with the decisions they rest on.

### The context

Every call carries the context, read from Core's records when the call is sent: the event's subject with the values the tell and Core's read already hold; Core's committed focus and its ancestors; the foreground and its application; the queue's facts (`pending.focus`, `pending.same-kind`, `pending.same-kind-same-control`, as NVDA's `isPendingEvents`); the keys (the toggles, and the modifiers as stamped at the event's arrival); the session (`unlocked`, `locked`, `secure-desktop`); and the module's settings and the Verbatim settings it declared it reads. Reason: under Core-calls-hooks each of these as a read would be two crossings for a fact Core already has. The nodes in it are named by the instance record for the call.

Every node field is filled only within the instance's scope, by the rule of section 5: a focus, ancestor or foreground in another application, or below the lock screen while Windows is locked, is `outside`, with no values [review M7]. The queue's facts are booleans about Core's queue and carry no content. A global module that declared `reads = ["focus", "foreground"]` gets those two and their ancestors in any application.

### Typed reads in one request

```wit
interface read {
  variant subject { node(borrow<node>), focus, foreground, window(window-path), earlier(u8), item(earlier-item) }
  variant op {
    properties(subject),
    related(tuple<subject, relation>),
    ancestors(tuple<subject, u32>),
    descendants(tuple<subject, walk>),
    test(tuple<subject, string>),
  }
  record step { op: op, set: list<property-id>, only-if: option<condition> }
  variant answered { value(property-value), absent, not-asked }
  read: func(steps: list<step>) -> result<list<outcome>, read-error>;
}
```

- One `read` is one `Wants` and one `Resume`, whatever its steps, up to sixteen. Each maps to the shared step list (`Read`, `Related`, `Ancestors`, `Descendants`, `Test`, with `Condition` for "only if"), so it adds no kind of request.
- The SDK's `#[derive(Properties)]` makes a struct one property set for one element step; the answer is parsed into it at the boundary. A required field answered `absent` is an error, an optional one `None`. `not-asked`, which appears only in context snapshots, makes the SDK read that property live, so a guest never takes "not asked" for "absent".
- Matchers travel as TOML text and are parsed by Core at the boundary; the SDK's `matcher!` macro parses them at compile time.
- `focus` and `foreground` as subjects are Core's committed focus and foreground.

### Text

A `read-text` request of steps over a node: `caret`, `expand`, `range`, `move`, `find`, `select`, `locate`, `visible`, mapped to `Caret`, `ReadText`, `ReadRange`, `Move`, `FindText`, `Select`, `Locate` and `Visible`. Points and anchors make a range a local value, so NVDA's `collapse`, `copy` and `setEndPoint` cost nothing. `select` is the one write, shown in the install dialog as "moves the caret in it". `MoveBySentence`, `WaitForCaret`, `CompareSelection` and `ReadColours` stay Core's, since they serve the hot path. Gaps, each a field or kind of an existing operation and so Dickson's (item 4): formatting on `ReadRange`, a case-insensitive `FindText` (NVDA's `find` defaults to case-insensitive, `textInfos/__init__.py` 582), and a comparison of two points that answers rather than skips.

### What a long read does

A read is the application's time, not the handler's: the call waits as the hook does, and the behaviours above apply unchanged. A read of a hung application answers `failed` when Core's foreground stop abandons the worker, as Core's own reads do.

## 13. Effects, settings and declarations

### Effects

`speak(message)`, `speak-node(node)`, `speak-spelling(text, descriptions)` (spelled by Core's pipeline with Verbatim's capitals and character descriptions), `cancel-speech()` (acts only while the module's application has attention, or during `on-command` until another key), `tone(hertz, ms)`, `browseable-message(title, text)`, `show-list-dialog(spec)`, `click(node, button)`, `copy-text(text, announce)` (Core writes the clipboard on a shell thread and checks it after, as NVDA's `api.copyToClip` does, announcing with NVDA's wording), `log(level, message)`, `set-own(id, value)`, and the reads `windows-version()`, `clock()` and, as a separate interface, `clipboard-text()`. Each effect is one item on the call.

Four effects have work of their own after their item arrives, and each needs an owner [review S4]:

- `speak-node`: Core's announcement reads what it needs (name, role, states, position) as Core's own request.
- `click`: Core reads the node's place through the outpost, then moves the pointer and clicks from a shell thread.
- `copy-text`: the shell step that writes the clipboard and checks it.
- `clipboard-text`: Core reads the clipboard on a shell thread and must answer the guest.

Recommended (question 4): while its call runs, such an effect's request is owned by the call's owner, and the call's `Final` waits for it, so the guest sees `copy-text`'s and `clipboard-text`'s results; the guest's import blocks until then. An effect whose call was cancelled is withdrawn with it. So no effect outlives its call, and no further owner is added. The answer to `clipboard-text` and `copy-text` is not a read of the shared step list, so `Resume` would carry a new kind of result: Dickson's (item 4). The other option, every such effect owned by the instance record after its call ends, lets a module's click run after the user has moved on.

### Settings

Decided (2026-11-28); the shape is `extension-api-additions.md` section 7's:

- `[[settings]]` in `module.toml` declares a module's settings: id, label message id, kind (`toggle`, `choice`, `number`, `text`), default and choices. Core owns them: the settings dialog shows them on a page under the extension's name, and the settings file keeps them under the extension and module ids. Reason: one owner, and module settings join the settings file's format, a public contract (item 1).
- `reads-settings` names Verbatim's settings by public names. The list of public names becomes part of the settings file's contract (question 3).
- Values travel in the context, so reading costs nothing and is fresh at every call. There is no settings-changed hook: a module sees a change at its next call.
- `set-own` changes one of the module's own settings, as a command that toggles it would. An extension never writes Verbatim's settings. Whether `set-own` is allowed at all is part of question 3.

### Declarations

Data Core applies on the hot path with no call: `sleep-mode` (sleep mode stays a Core per-application list, seeded by the manifest, as NVDA's `sleepMode` is an `AppModule` attribute toggled by a core command); `[typing-echo]` (Calculator's "edit controls only means always", bound to a module setting by `when`); and the rules. Reason: anything a hook would decide on the hot path is expressed as data instead.

## 14. State and persistence

- An instance's memory is its state, one instance per process for an app module, as NVDA keeps one `AppModule` per process. It is lost at reload, at the instance's end (a trap, the compute bound, the memory limit, an abandoned lane), at the host's end and at the process's exit; a cancellation keeps it (section 11). Reason: NVDA's modules hold small state (Calculator's one flag) and rebuild it; persistence is explicit.
- Persistent state is the extension's data folder: read and write, with a size cap, its paths resolved inside the folder, no links created, and the container granted only the extension's own folders (security model, X14). Under `wasm32-wasip2` the natural form is a `wasi:filesystem` preopen of the data folder alone, which `wasmtime-wasi` resolves through `cap-std`; the 2026-11-09 design names `cap-std` as the resolution to check from its source before adopting. A secret store for API keys sits outside the settings file (roadmap, M13). Both are in M13 by the roadmap; whether they come with the rewrite's first extensions is question 6.
- Nothing about an instance is persisted by Verbatim itself.

## 15. Strings, translation and phrases

- An extension's strings are Fluent files under `locale/<language>/`, English the fallback, and the language follows Verbatim's interface language. Every user-facing text in a manifest (names, descriptions, command descriptions, setting labels) is a message id.
- `speak(message)` takes text read from the application or a message id with arguments. Reason: a module's own words are then translatable without code changes.
- A built-in's strings are Verbatim's own Fluent resources in `verbatim-i18n`. A built-in that ports an NVDA behaviour uses NVDA's English text, so the oracle comparison matches word for word, and NVDA's spoken vocabulary is matched for familiarity. NVDA's non-English translations are never imported (CLAUDE.md, NVDA provenance).
- Translating speech is a different matter, the speech-text stage of section 9: it receives each utterance's text segments with their language tags and returns new ones.

## 16. The API as a public contract

The extension API is Dickson's (item 1).

- The WIT files in the API crate are the single source: `wasmtime::component::bindgen!` generates the host's traits, `wit-bindgen` the guests' bindings, and an xtask step generates the guide's API reference, which `cargo xtask ci` checks is current; the manifest reference and a JSON Schema are generated from the manifest types. The conversions between internal types and WIT types are exhaustive matches, so a new internal variant fails to compile until someone decides how modules see it.
- The WIT package carries a semantic version, every import names it, and the linker resolves an import by exact name, then by track, several tracks living side by side for a deprecation period; `wasm-tools component semver-check` proves each new version compatible. The manifest's `api` must agree with the binary's imports, which are the authority. There is no minimum or tested Verbatim version, since the tracks already say what an extension needs (Dickson, 2026-11-09).
- Recommended policy (question 3): the API stays at 0.x, with no compatibility promise to third parties, until the inbox built-ins are ported and the first measurements are in; 1.0 is then frozen, with additions as new minor versions and breaking changes as a new track kept beside the old.
- Updates of installed extensions: an update is installed as an install is, with only the new flows shown for approval. The updater itself is M8.6's.
- Guests: built-ins and the SDK target `wasm32-wasip2` (ruling). Rust's standard library on that target imports WASI interfaces, so the host serves a virtual WASI from `wasmtime-wasi`, configured with no environment, no inherited standard streams and no preopens unless the data folder is granted, with clocks and random sources that Verbatim supplies, so the simulator can serve them deterministically. Each WASI interface is mapped to a capability row or refused. Claude's proposed mapping, a Decision for Dickson (question 7): clocks, random, environment and exit serve Verbatim's values and show nothing; standard output and error go to the log under the module's id and show nothing; `wasi:filesystem` is the data folder and shows nothing; `wasi:sockets` is refused; `wasi:http` outgoing is the network import, checked against the manifest's hosts, with the network warnings. This replaces "every WASI interface is refused" and the `wasm32-unknown-unknown` target of the 2026-11-09 design.

## 17. Verification

- The simulator: a host is a peer under the seeded scheduler like any other, its root reducer and lanes run as they do in the product, and the guest runs for real in Wasmtime, deterministic given its imports, with time, randomness and the network served by the simulator. Its reads are answered through Core from the world, so a different path through the module still gets answers. Faults: slow answers, a trap, a lane stuck in native code, the host's end at any step, the breaker. Invariants, besides the skeleton's: an instance holds no id beyond what Core's records name, and at quiet Core's `held` equals the host's last `Holding` report; every call, read and instance has one ending, and an instance's end resolves every call due or running on it; no effect is applied after its call's ticket resolved; a holding call never waits behind a non-holding one beyond that call's computation to its next import; no `on-node` call is owed for a `Wants` or record-owned read; nothing below the lock screen reaches a guest while Windows is locked; an unhandled event has no extension stage; and a lane runs one call at a time. Cancellation is cooperative, so the replay needs no model of epoch timing for it [review S2].
- The oracle, for built-ins: a built-in that ports NVDA behaviour joins the comparison corpus with NVDA's own app module as its oracle, loaded in the headless harness by executable name through the replaced `getAppNameFromProcessID` (`appModuleHandler.py` 145; `design.md`, M5). A built-in global module that ports a `globalCommands` script (the time and date at 486, Report focus at 2956, Report title at 3210) is compared with that script. The tray and taskbar lists port an add-on outside the pinned source, so they have the add-on run headless on the same world, optionally, or transcripts Dickson approved.
- Third-party extensions: replayed in the simulator and verified by the contract checks, the invariants and approved transcripts; where an extension ports an NVDA add-on, the add-on can run headless on the same world as an optional comparison (Dickson, 2026-11-28).
- Contract tests: a test component per import and export round-tripping every type; refusal of each import outside the world, a WASI interface outside the mapping included, with the exact message; capability derivation (every import maps to a row or to nothing, as a generated test); the matcher grammar and the rules' layering as generated tests; the package reader against paths that escape.
- Live tests: the host's container, memory cap and Wasmtime on ARM64 run in the host's own tests and on `ci-arm64`. End to end, one scenario per ported behaviour asserts its captured utterance.
- The dependency: linking Wasmtime into the simulator, so built-ins run in replay tests on `ci-linux` too, is the same dependency used in test tooling. Claude's, unless Dickson counts it as a new adoption.

## 18. Performance

Measured in this repository:

| Figure | Value | Source |
|---|---|---|
| Core to an outpost and back, one request | 1.0 to 1.5 ms; 3.0 ms once with the worker busy; debug, logging on | `books/dev/src/reference/performance.md` 155 to 164 |
| Core's own turn in that | 0.20 to 0.27 ms | the same |
| A return to an application walked live, key or action to speech queued | median 0.8 ms, 99th percentile 11.6 ms, debug | `performance.md` 322 to 324 |
| The budget | 10 ms from the operating system's event to the utterance queued (D15) | `books/dev/src/explanation/architecture.md` 141 to 146 |

Not measured: any Wasm call, host turn, guest time, instantiation, host memory, a host process's start in its container, a cold compile, or the size a registration adds to an event's cache request.

Estimates, by counting messages and using the outpost crossing as a stand-in [review S9]:

| Case | Crossings | Estimate added to the path |
|---|---|---|
| An event no module registered | none | one index lookup in Core, well under 0.1 ms |
| A rule or declaration | none | a table lookup |
| A hook using only the context | one, Core and host | about 1 to 1.5 ms plus guest time |
| Each read | two, host to Core and Core to the outpost | about 2 to 3 ms plus the application's own call |
| A focus with one hook and one read | three | about 3 to 4.5 ms |
| A hook making three reads one at a time | seven | about 7 to 10 ms, missing the budget, which is why reads are batched |
| An instance's first call | one, with `Instance` in the same write | the crossing plus instantiating the pre-linked component, not measured |
| A call to an installed extension whose host is not running | the host's start, then the call | a contained process's start, not measured; a holding hook at a cold start waits for it, ending on its causes |
| A `not-asked` field the SDK reads live | two per such read | as a read; shown in the timeline as the SDK's |

What the host does to stay fast: warm instances kept between calls; compiled code pre-linked in the content-named cache; the built-in host always running; one pipe write per guest run; the context and batched reads. The first build of the host measures the crossing, the host turn, instantiation, guest time and a contained host's start on both architectures, and the instance memory with many outposts open, and sets the compute bound and the memory limits from them. The built-ins' figures are then listed per registration in the ledger check of section 8.

## 19. Worked examples

### Report title, a built-in global module

NVDA's `script_title` reads the foreground's name, falls back to the application's name and then "No title", refuses on the lock screen, speaks on the first press, spells on the second and copies with notice on the third and later (`globalCommands.py` 3210 to 3230).

- Manifest: `kind = "global"`, `reads = ["focus", "foreground"]`, three commands on `verbatim+t` with `presses` 1, 2 and 3; a fourth press runs the third again, as NVDA's repeat count of 3 or more copies.
- Code: from the context, the foreground; when it is `outside` because Windows is locked and it lies below the lock screen, speak "Action unavailable while Windows is locked"; otherwise one read of the foreground's name, or none if the context's snapshot holds it; then `speak`, `speak-spelling` or `copy-text(text, announce = true)` by command. The code is `extension-api-additions.md`'s, with `outside` in place of none.
- Cost: none or one read, so one to three crossings (estimate 1 to 4.5 ms). Does not hold the queue; its speech is judged by the key fence. Compared with `script_title` in the oracle.

### File Explorer's tab switch

NVDA's `explorer.py` 499 to 517 speaks a tab as a focus when it is selected, its parent's automation id is `TabListView` and no other selection event is pending, then calls `nextHandler`. The parent is a live read in NVDA too (`obj.parent`).

- Manifest: a holding `on-event` registration, `kinds = ["selection"]`, `backend = "uia"`, matching `control-type = "TabItem"` and `states = "selected"`, both added to the selection event's cache request, so the match makes no call [review M8]. The parent's automation id is a relation and cannot be matched.
- Code: if `ctx.pending.same-kind`, pass; otherwise one read of the parent's automation id; if it is `TabListView`, `speak-node` of the context's subject; pass.
- Cost: on a selection of a tab in Explorer only, one call, one read and the announcement's own read: five crossings, about 5 to 7.5 ms (estimate). A selection of anything else, one per arrow key in a file list, costs the lookup and the two cached properties' share of its event. Compared with `explorer.py` in the oracle.
- The rest of Explorer is in section 20.

### Calculator

NVDA's `calculator.py`: the digit keys pass through and set a flag (185 to 190); Enter, Escape and Delete pass through (143 to 155); `event_nameChange` (72 to 92) handles every UI Automation name change in Calculator, speaking a changed name unless it is an entry or the result, which it caches; `event_UIA_notification` clears the flag on every notification (99 to 100) and, for `DisplayUpdated`, finds `LandmarkTarget` from the notification's own window and either says nothing (an entry field first and the flag set) or cancels speech and speaks the result child's name (94 to 141); typing echo set to "edit controls only" counts as always (61 to 70) [review S8].

- Manifest: a `digit` command on the digit keys with `pass-through = true`; a holding `on-event` for every `name-change` in Calculator, its automation id and UIA class name in the cache request; a holding `on-event` for every `notification`, with `ends-on = ["later-key"]` and `on-cancel = "drop"` (section 11); a `[[settings]]` toggle for the typing echo, on by default to match NVDA; and a `[typing-echo]` declaration bound to it.
- Code: the command sets `no_result_key`; the name-change hook applies NVDA's cache and its rule; the notification hook clears the flag, and for `DisplayUpdated` makes one read (a find of `LandmarkTarget` under the notification's window with its children's automation ids and names), then drops, or cancels speech, speaks the result and drops, or passes.
- Order: the digit's call is made due before the events it causes, so the flag is set first; neither the digit key nor its echo waits for the host.
- Cost per digit, estimate: the command's call, one name-change call per changed name (no read, since the tell carries the name and the cached ids), and the notification's call and read: at least five crossings, about 5 to 7.5 ms, only in Calculator. The command computes and makes no import, so it never delays a holding call.

### A translation add-on

A third-party speech-text stage that translates what Verbatim says through an online service. In NVDA such an add-on registers on `filter_speechSequence`, which runs synchronously in `speech.speak` (`speech/speech.py` 1137), so a slow service delays that speech for as long as it waits.

- Manifest: `kind = "speech-text-stage"`, taking announcements, messages and text read and leaving typed characters and spelling; settings for the target language and the service's address; the network host in the manifest; a failure indication "Translation failed".
- Code: `transform(ctx, utterance)` sends the text segments to the service through the network import and returns the translated segments with their language tags, or the failure indication on an error.
- Behaviour: the speech root holds each taken utterance for its translation, and later taken utterances behind it; untaken utterances, Verbatim's own dialogs and its messages about extensions bypass the stage, and keys and events never wait. A key press cuts the pending utterances and cancels their calls, and a cancelled call keeps the module's cache.
- Install dialog: "Changes what Verbatim says before it is spoken", and the warning "Can send the text Verbatim speaks to <host>".
- Cost: the service's own time per taken utterance, plus one crossing (estimate 1 to 1.5 ms). Such an event exceeds the 10 ms budget by that cost, as ruled. Replayed in the simulator with the network served by the world; not compared with NVDA.
- Needs: the speech-text stage kind and the network import, both M13 by the roadmap (question 6), and an answer to what gives speech back from a service that never answers (question 5).

### A third-party app module

An illustration, not NVDA behaviour: a module for a music player, "Example Player", that speaks the new track when it changes, with a setting to switch that off and a command for the elapsed time.

- Manifest: `kind = "app"`, `applications = ["exampleplayer"]`; a following `on-event` (`follow = true`) for `name-change` matching `automation-id = "NowPlaying"`, added to the name-change cache request; a `[[settings]]` toggle `announce-tracks`; a command `elapsed` on `verbatim+shift+e`.
- Code: the event hook speaks the event's new name when the setting is on; the command makes one read of the `Elapsed` text's name and speaks it.
- Behaviour: the track change never delays the player's other events, and its speech is queued, not interrupting, when the player is not in front. While the focus is in another application, the context's focus and foreground are `outside`, so nothing of that application reaches the module [review M7].
- Install dialog: "Example Player: changes how Verbatim reads it", and the command with its gesture and any conflict.
- Verification: replayed over a recorded world of the player with the transcript the author approved; not compared with NVDA.

## 20. The built-in modules

From `extensions-first.md`, section 1, checked again against `explorer.py` [review S8].

App modules (NVDA's `appModules/`):

- File Explorer (`explorer.py`):
  - Rules: the shell staging windows' silence (`ForegroundStaging`, `LauncherTipWnd`, `ApplicationManager_DesktopShellWindow`) as `silent`; the snap results notification as an event `accept`; good-UIA shell roots as `backend`; the search band's MSAA focus as a veto; the Alt+Tab switcher's item containers suppressed as a node rule on `alt-held`, and its frame's focus as a veto.
  - Hooks: tab switches (above); the menu bar's exit focus and the notification area's focus, each dropping on `pending.focus` (section 11); the notification area toolbar's redirect to its focused child, which reads the children's focused state.
  - The overlay classes between lines 158 and 213 (`StartButton`'s states, `UIProperty`'s value, `ReadOnlyEditBox`'s window text, `MetadataEditField`'s text, `WorkerW`'s focus), each checked against the rule actions when the module is ported; one that needs more than a rule becomes an `on-node` hook or a gap reported then.
  - The status bar, as `status-bar`.
  - Not ported in the first set, with their reason: `event_UIA_window_windowOpen` (485 to 497), which re-sends the event on a child in the input panel's application, so it needs a verdict that hands an event to another application's module and lands with that module (the emoji panel, waiting for IME support); `ExplorerToolTip` (116 to 152), which needs the mouse's position and last move, M13's; the notification area's mouse move out of the way, the same.
  - The duplicate list-view focus (`SysListView32EmittingDuplicateFocusEvents`, 56 to 72) is not a hook: NVDA judges it before queueing against `lastQueuedFocusObject`, which is Core's duplicate-focus test, so it is verified against a capture as Core's behaviour, and a rule is added only if Core's test does not already drop it.
- Notepad (`notepad.py`): tab switches, the status bar, `LineNumberBox` as an edit (a node rule).
- Calculator (`calculator.py`): history and memory item names from their children (`on-node`), the name-change hook, the notification hook, the pass-through result keys, the typing-echo declaration.
- Settings (`systemsettings.py`): volume meters are not progress bars, a node rule.
- Task Manager (`taskmgr.py`), Quick Settings (`shellexperiencehost.py`) and Open With (`openwith.py`).
- Start and search (`searchui.py`): its suggestion names and ancestry are rules and `on-node`; its Chromium document's focus veto (32 to 43) needs a live read of an ancestor's focused state before the focus is accepted, which has no form under ADR 0003's commit before hooks (question 8).
- Windows Terminal: its second identity as a focus veto in the listener dropping every `TermControl` focus whose runtime id starts 14 (Dickson, 2026-11-09). The terminal behaviour itself stays Core.

Global modules (commands that need only the API and are not the hot path):

- The system tray and taskbar lists (Verbatim+F11, once and twice), NVDA's `systrayList` add-on (Dickson, 2026-10-31 and 2026-11-03), reading Explorer by `reads`.
- Speak time and date (Verbatim+F12), NVDA's `script_dateTime` (Dickson, 2026-10-31).
- Report focus (Verbatim+Tab), NVDA's `script_reportCurrentFocus`, with spelling on the second press, reading by `reads = ["focus"]`. It moves out of Core, which the rewrite rewrites anyway.
- Report title (Verbatim+T), NVDA's `script_title`, not built today, reading by `reads = ["focus", "foreground"]`.

Synthesizers: eSpeak NG and OneCore, each a native module in a host of its own (Dickson, 2026-11-09).

Core, not modules: sleep mode's list (seeded from manifests), the review cursor, object navigation and say all, Report status bar's terminal rows and previous and next command, toasts, and every hot-path step.

## 21. Size, estimated

| Part | Lines |
|---|---|
| API crate: WIT, manifest types and validation, matcher parser, rule types | 2,500 |
| Host program: root, lanes, Wasmtime embedding, virtual WASI with the `wasi:http` host allowlist, the imports' host side, store limits, the abandoned lane | 3,300 |
| Core: `Extensions`, the loader, the registration index and cache-request growth, the instance records and their order rule, effect judging, the breaker, settings pages, the install and Extensions dialogs | 4,000 |
| The speech root's stage step | 300 |
| Guest SDK: bindings, `Properties` derive, `matcher!`, request builders, test helper | 2,000 |
| The package installer with its staging and path checks, the compiled-code cache, the supervisor's container and caps for hosts, the secure-screen allowlist (M8.7) | 1,400 |
| Product machinery total | about 13,500 |
| Built-ins: Explorer about 500, Calculator about 200, Notepad about 100, the five other app modules about 300 (NVDA's five are 312 lines), the four global modules about 400 | about 1,500 |
| The simulator's host peer and its network world, the generators for registrations and modules | 1,200 |
| Tests: the contract tests of section 17, the record tests, the host's tests | about 7,000 |

NVDA's counterparts, counted with `wc -l`: `appModuleHandler.py` 952, `globalPluginHandler.py` 93, `addonHandler/` 1,450, `extensionPoints/` 505, about 3,000, and the add-on store with its dialogs (`addonStore/` and `gui/addonStoreGui/`) 6,512, the install dialog's counterpart: about 9,500 in all [review S10]. The machinery is about 1.4 times that. Without the store and Verbatim's installer it is about 12,100 against 3,000, about 4 times, because of the sandbox, the process split and the typed contract, none of which NVDA has. The built-ins are about NVDA's modules (`explorer.py` 551, `calculator.py` 190, `notepad.py` 84). A figure to report, not a gate. The tests are about half the product machinery; the contract suite is most of it.

## 22. What changes in design.md

- Terms and peers: extension hosts are peers from the first build that needs a hook, not "from M5".
- Protocol: the `Instance` standing step with its `Holding` item, beside `Call` and `Resume`; `Call` naming its instance; a new kind of result on `Resume` for the clipboard's answer, if approved.
- Records, Core: `Extensions` with its five-state value; an instance record per claiming module in `Application` with its four variants and the order rule, holding the list-dialog record now listed there; global instances in `Active`.
- Records, hosts: `HostRoot` and `Lane` with its `Running` value, and no call queue.
- Requests a record owns: one new owner, the instance record (non-holding calls and their reads and effects), Dickson's (rule 1's closed list).
- Queue: the stages `ext.call` and `ext.read`, with each registration's declared end causes and cancelled outcome; the context sent with each call, filtered by scope.
- Rules: the listener's part in the `Watch` filters, which also carry each kind's cache request grown by registrations; every other rule and every hook registration matched in Core.
- The speech root: a stage step in its pipeline, and the speech root as an asker of stage hosts, beside the synthesizer's (Dickson's, items 3 and 4).
- M5 section: Core matching registrations; the costs of section 18; one read outstanding per instance, pending Dickson; the guide's "Where hooks run" and its single host clock replaced by section 11; the lock-screen rule at every send and arrival.
- Tiers: built-in guests in a GPL guest tier, free to port NVDA's app modules, outside the product's tier check; the SDK and API crate original (ruling).
- Verification, "Extensions": built-ins compared with NVDA; third-party extensions replayed only; Wasmtime linked into the simulator; the invariants of section 17.
- Adding a feature: two forms, a Core feature as today, or a module folder with `module.toml`, its guest crate if it has code, its strings, a world with NVDA's module or script as its oracle, and its parity entry.
- Build order: rules and the lifecycle land with the readers; the host, with one built-in hook and the measurement of a crossing, instantiation and guest time against D15, comes before any built-in with a hook is ported.
- Sizes: section 21's lines added.

## 23. Open questions

Each with its options and a recommendation. Questions 1 to 9 are Dickson's; 10 to 16 are Claude's, listed so that he can reverse them.

Status (Dickson, 2026-11-28):
- Decided: question 1, Core calls every hook; question 4's first two points, the `Instance` standing step with its `Holding` item and the instance record as an owner of requests.
- Provisional for the prototype: questions 2, 3, 5, 6, 7, 8 and 9, question 4's remaining points (effects owned by their call with the clipboard's result kind, and the speech root asking stage hosts), and the pending `on-node` parity difference (section 8), each on its recommended default. Each comes back to Dickson when its feature or third-party extensions arrive, and all before the API is frozen at 1.0.
- The prototype's first extension steps: Report title, a command making one read; then File Explorer's tab switch, a hook (section 19). Report title proves the host, the `Instance` and `Call` steps, the context and one `Wants` with no queue held; the tab switch adds a holding hook, its end causes, the cache request and the crossing's measurement against D15.

1. Who calls hooks. Decided: Core (Dickson, 2026-11-28).
   - Core calls every hook, and reads go through Core (ADR 0003). One crossing per call, two per read; Core sees the queue's facts and decides what waits.
   - The outpost calls hooks (the 2026-11-09 design). One crossing per read, but the outpost has none of Core's facts (the pending focus, the keys, the lock state, the settings), so Core would send copies for a reader to judge, the copied-state class the ownership refactor removed; and the outpost waiting on a host is what ADR 0003 ruled out.
   - Core calls, and the host reads from the outpost directly under a scope Core grants. Halves each read's cost, but the outpost then serves two askers with no order between them, and the ids a host gets back are in no Core record, so the held view cannot name them.
   - Recommended: Core calls, with the context and batched reads keeping reads few; the third option is the fallback if a measured built-in misses its budget on a hot-path event.
2. The trust model (item 2):
   - Options A, B and C of section 5; recommended A.
   - The lock screen: nothing below it reaches an extension while Windows is locked, applied at every read, context, event and effect (recommended); or the node with a property.
   - The context filtered by the instance's scope (recommended), with `reads = ["focus", "foreground"]` as a declared, shown scope.
   - Commands on unmodified keys shown as "Sees what you type" (recommended), or refused for global modules.
   - Report title, Report focus and the tray lists reading by that narrow scope (recommended), or reading every application brought forward from M13.
3. The API as a contract (item 1): the context argument; typed reads and the text interface, with the three request-list gaps; the sixteen event kinds and `activity-id` in hook registrations; matchers limited to what an event carries, with cache-request growth; `after-focus`, `after-event` and `follow = true`; `ends-on` and `on-cancel`; `speak-spelling`, `copy-text` and `clipboard-text`; `[[settings]]`, `reads-settings` and its public names, and whether `set-own` is allowed (recommended yes, since a command that toggles its own setting is common in NVDA add-ons and Core still saves and shows it); the new capability rows; and the version policy, 0.x until the inbox built-ins are ported (recommended). Options: approve as one set, or interface by interface as the 2026-11-09 work plan had it.
4. The protocol and processes (items 3 and 4); the first two points decided as recommended (Dickson, 2026-11-28), the rest provisional:
   - The `Instance` standing step with `Holding` (recommended), or an implicit lane.
   - The instance record as an owner of requests on the closed list (recommended), or every non-holding call as a queue entry, which would put a host on the path of every later key.
   - Effects with work of their own owned by their call, the call's `Final` waiting for them, with a new kind of result on `Resume` for the clipboard's answer (recommended); or owned by the instance record after the call.
   - The speech root as the asker of stage hosts (recommended), or Core holding utterances before `Say`, a second speech queue.
5. Long-running handlers:
   - The behaviours per hook as section 11's table, with `ends-on = ["later-key"]` and `on-cancel = "pass"` as defaults (recommended); or Hold for every holding hook as NVDA does; or a clock bound added as a user setting.
   - A holding call due while a non-holding call of its instance runs cancels that call (recommended); or waits for it, which is unbounded; or runs on a second instance, which loses shared state.
   - A compute bound on guest processor time per call, measured and generous (recommended), or none.
   - One read outstanding per instance (recommended, no state), or per extension as ADR 0003 rules (a waiting list in the extension's record).
   - A late following hook's speech: spoken while the focus it followed stands (recommended), or dropped once later speech is queued.
   - What gives speech back from a speech-text stage that never answers: a user setting per stage, "speak the failure indication after N seconds", with a default Dickson sets (recommended); or nothing but cancel speech and the Extensions dialog, which now works since its speech bypasses stages; or a fixed bound.
6. Scope (item 5): which of M13's items come with the rewrite's first extensions: the speech-text stage kind, the network imports, the data folder and secret store, and the timer. Recommended: the stage and the network imports, since the translation example and the long-handler rulings rest on them, with the data folder because the network's API keys need the secret store; the timer stays M13's.
7. WASI under `wasm32-wasip2` (items 1 and 6): the mapping in section 16 (recommended), or a smaller set refusing `wasi:http` in favour of Verbatim's own network interface; and `wasmtime-wasi` as the implementation, part of the Wasmtime dependency already decided, with Tokio inside it read from its source first.
8. A focus veto that needs a live read before the focus is accepted (Start search's Chromium document, `searchui.py` 32 to 43), against ADR 0003's "a focus is committed before its hooks run": a holding `veto-focus` hook before the commit (a new hook and an ADR change), or a rule action that reads an ancestor's live focused state in Core's focus read (a new property in the read), or the behaviour left out with a parity entry. Recommended: the rule action, since the focus read already walks the ancestors and it adds no hook to the hot path.
9. Which process registers the extra UI Automation events, desktop-wide with a process filter or per window of the claimed application (item 3). Recommended: desktop-wide in the listener while a module registers the kind, after measuring the event rate on a busy desktop, since one registration is simpler than one per window and the listener already filters by process.
10. (Claude) Core matches every hook registration; the listener applies only drop and veto rules.
11. (Claude) Cancellation is cooperative only, with no forced re-creation of an instance.
12. (Claude) A call due during the compile waits in the instance record's `Pending` variant; a failed compile resolves it `Ended`.
13. (Claude) The lane's grace after a cancellation, 1 second proposed, measured before it is fixed.
14. (Claude) An installed extension's host is started when first needed and ended when no instance is open; the built-in host always runs; three ends of the built-in host in a minute with no module named turn the built-ins off for the session.
15. (Claude) Wasmtime linked into the simulator and built on `ci-linux`.
16. (Claude) A speech-text stage's registration names the utterance kinds it takes, and Verbatim's own interface speech never passes through a stage.
