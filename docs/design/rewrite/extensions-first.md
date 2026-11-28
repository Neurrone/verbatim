# Extensions first: the line between Core and built-in extensions

Status: design study, Claude, 2026-11-28. Nothing in the repository changes because of it. It applies Dickson's decisions of 2026-11-28: the rewrite includes M5; behaviour NVDA would put in an add-on is written as an extension from the start; Core is GPL tier and may port NVDA's code; and the two rulings below on the line itself. It reads against `design.md` (this folder), `docs/design/extensions.md`, `docs/design/m5-extensions-work.md`, the Extensions guide's reference (`books/extensions/src/`, settled 2026-11-09), the roadmap's M5 entry (`docs/roadmap.md` 864 to 1048) and ADR 0003's extensions rulings (`books/dev/src/reference/adr/0003-readers-tell-core-asks.md` 121 to 128, 249, 257). NVDA is read at the pinned commit 732d1ce2f. Every point marked "Decision for Dickson" waits for him; every other choice is Claude's.

## Assessment first

- The rule holds up against NVDA's own structure, with one reading that needs stating: NVDA's add-ons are unrestricted Python and can replace anything, so "could implement" has to mean "could implement through Verbatim's extension API, as NVDA's add-ons do through `appModuleHandler.AppModule` and `globalPluginHandler.GlobalPlugin`", and NVDA's own placement settles the doubtful cases: what NVDA puts in `appModules/` is an extension, and what it puts in `eventHandler`, `api`, `NVDAObjects`, `review`, `speech`, `sayAll` and the handlers is Core or the hot path.
- Most of today's features stay in Core, because they are on the hot path or are what add-ons build on. The extensions are the Windows inbox applications' behaviour (NVDA's app modules) and a handful of commands NVDA has in `globalCommands.py` that use only public reads: Report focus, Report title (not built today), the time and date, and the tray and taskbar lists (an add-on in NVDA).
- The tension is that NVDA's app modules change focus speech, which is the hot path. The study's answer is that an extension changes the hot path only by data (rules and declarations) applied in Core and the readers, with no crossing, and that a hook on a hot-path event is allowed only where registered narrowly and measured. That is a decision for Dickson.
- Cost: no Wasm call has been measured in this repository. By the one crossing figure measured (Core to an outpost and back, 1.0 to 1.5 ms in a debug build with logging), a hook with no read adds about one such crossing and each read two, against D15's 10 ms. The measurement in M5's step 4 is still the first thing to build.
- The settled reference conflicts with ADR 0003 in two places (who calls hooks, whose clock bounds them). ADR 0003 is the later ruling, so the guide's text is corrected, not reopened. Twelve gaps or questions follow, nine of them Dickson's.

## Dickson's rulings applied (2026-11-28)

- Anything an NVDA add-on could implement is an extension; what add-ons build on stays in Core (Dickson, 2026-11-28).
- Anything on the hot path stays in Core, whatever an add-on could do (Dickson, 2026-11-28, corrected the same day). The hot path is the latency-critical path every key or event takes on its way to speech. It is not every command a key triggers: a command such as Report title, though started by a key, is an extension if an add-on could implement it.

## The hot path, defined

The budget it must meet is D15 (`books/dev/src/explanation/architecture.md` 141 to 146): from the operating system's event, a key press or an accessibility event, to the utterance queued, 10 ms or less on every backend; and from queued to the first audio sample, 10 ms or less with eSpeak NG, OneCore exempt. Today's one measured figure for a whole path: a return to an application walked live, key or action to speech queued, median 0.8 ms and 99th percentile 11.6 ms (debug, `cargo xtask measure`, 2026-11-08; `books/dev/src/reference/performance.md` 322 to 324).

The hot path is these steps, and nothing else:

1. Key intake. The keyboard hook's judgement (hold, pass or swallow), the gesture's resolution against the published map, the multi-press stamp, Core's line of keys (`Keys` in design-v2), the key fence that cuts speech, and cancel and pause.
2. Caret and typing. A caret key held, passed after its read or at 150 ms, the outpost's caret watch, the caret read and the line, word or character spoken; selection with Shift; Backspace, Delete and Enter in a text field; typed character and word echo, terminals' typing echo included.
3. Event intake. The listener's tell, coalescing and credit, the queue's placement and limiter, end causes, and the foreground stop.
4. Event speech. Focus judging (validity, duplicates, pending focus), the focus read with its ancestry and line join, the announcement's composition (role, states, position, name, value, description), and Core's generic handling of every event kind it speaks: name, value, state and selection changes, notifications, toasts, progress bars, menus, alerts and live output in terminals.
5. The speech pipeline. `Say`, the speech manager, the synthesizer, the mixer and the audio device.

Not on it: a command's own work after its key is resolved (step 1 is on it, the command's entry is not), the dialogs and lists a command shows, and reports the user asks for.

One exception already stands: the synthesizer is on step 5 and runs as a built-in extension in a host of its own (eSpeak NG and OneCore as native synthesizer modules, Dickson, 2026-11-09; `extensions.md` section 12). The hot-path ruling is read as leaving that decision as it is, since a synthesizer is an output driver rather than Verbatim's judgement. Decision for Dickson, to confirm.

## 1. The line between Core and extensions

### Tested against NVDA's own structure

- Events. `eventHandler._EventExecuter.gen` (`eventHandler.py` 140 to 165) runs each event through the global plugins, then the focus object's app module, then the tree interceptor, then the object itself, each able to stop the chain by not calling `nextHandler`. The object's own handler is NVDA's base behaviour: `NVDAObjects`, with classes such as `behaviors.ProgressBar` (41), `LiveText` (372), `Terminal` (511), `ToolTip` (985) and `Notification` (1002), and `NVDAObjects/UIA/__init__.py`'s `Toast_win8` and `Toast_win10` (2811, 2815). That base is Core under both rulings: it is what add-ons build on and it is event speech.
- Scripts. `scriptHandler._yieldObjectsForFindScript` (`scriptHandler.py` 161 onward) yields the gesture's own object, the global plugins, the focus's app module, the braille display, vision providers, the tree interceptor, then the focus ancestors and `globalCommands`. So an add-on can bind any gesture, ahead of NVDA's own commands. The commands in `globalCommands.py` are NVDA's core, but many use only what a global plugin can reach.
- Application behaviour. NVDA ships 78 app modules in `appModules/` (`extensions.md`, appendix A) and no global plugin (`globalPlugins/` holds only `__init__.py`). Every app module is written against the same `AppModule` class an add-on's is: `chooseNVDAObjectOverlayClasses`, `event_NVDAObject_init`, `event_*` handlers with `nextHandler`, scripts, `isGoodUIAWindow`, `isBadUIAWindow`, `shouldProcessUIANotificationEvent`, `sleepMode` and `statusBar` (`appModuleHandler.py` 494 to 879).
- So NVDA draws the same line the ruling draws, with one difference: NVDA keeps its cross-application commands in `globalCommands.py` even where a global plugin could hold them. Under the ruling those go to a built-in global module when they need only the extension API and are not hot path.

The test, as applied below: a behaviour is an extension when NVDA implements it in an app module, or an add-on implements it, or it is a command that needs only the extension API's reads and effects; it stays in Core when it is on the hot path, when NVDA implements it in `eventHandler`, `api`, `NVDAObjects`, the API handlers, `review`, `speech`, `sayAll`, `braille` or the keyboard handler, or when it owns state other features read.

### Every current feature, classified

From `books/dev/src/reference/test-coverage.md`, row by row, grouped.

Core, on the hot path:

- Shell and input: lock keys, the Verbatim key's double tap and choices, the menu guard, cancel speech, pause speech, the speech interrupt settings, editing chords, moving by sentence, clearing keys, repeated presses, the laptop layout, elevated applications' keys (step 1).
- Text and editing: caret keys and the caret tell's kept element, selecting with Shift, Backspace and Delete, Enter in a text field, typed character and word echo, typed characters read-only, holding caret keys, protected fields, line breaks, word and character segmentation (step 2).
- Terminals: new output and floods, typing in terminals, alternate screens, lists and progress lines, windows and tabs, console windows (steps 2 and 4; NVDA's `winConsole`, `winConsoleUIA` and `behaviors.Terminal` are its core).
- Focus and announcements: focus announcement, a dialog read once shown, focus ancestry, duplicate and ordered focus events, the known ancestors and the readers' last focus, the startup announcement, menus, selection and controlled lists, a combo box opened, state changes, value changes and sliders, UIA notifications, progress bars, a value change by the background setting, help balloons (NVDA's `ToolTip` and `Notification` from `tooltips_class32`, `NVDAObjects/IAccessible/__init__.py` 2845 and 2846), table cells, native controls and their window messages (step 4).
- Toasts. Borderline: a global plugin could catch `UIA_window_windowOpen` and speak them, but NVDA puts them in its UI Automation object layer and they are event speech, so Core.
- The ghost window, the foreground UIA progress bar not the focus, hung and slow applications, window destruction, a window's backend (step 3 or 4).
- Speech and synthesizers: lanes, cancel and marks, themes and sounds, spelling, capitals and character descriptions, synthesizer switching (step 5); the drivers are the synthesizer exception above.

Core, because add-ons build on it (not hot path):

- The review cursor and object navigation, with their state: the navigator, the review position, the start marker. NVDA's `review.py` and `api.getNavigatorObject` are what add-ons build on. Their commands (top, lines, words, characters, selection start and end, caret and review location, page up and down, parent, siblings, first child, to focus, activate, tree view navigation, set and move to the start marker, select then copy) are borderline: an add-on could re-bind each, but each moves Core's own state. Decision for Dickson, below.
- Say all, from the caret and from review, and its settings (`sayAll.py` is NVDA's core, and its reads ride the speech pipeline).
- The desktop's return and the virtual desktop's name (`eventHandler.py` 43 to 46 and 328 to 368), the secure desktop and the lock state.
- Settings and the settings dialog, persistence, themes, the toggles of hot-path features (Verbatim+2, +3, +5, +6), the Verbatim menu (Verbatim+V), quitting with dialogs open, and every recovery and lifecycle row (starting, replacing an instance, the settings file, the screen reader flag, crashes, idle outposts, generations, batches, the flight recorder).
- Report status bar (Verbatim+End) as Verbatim builds it today: it reads a terminal's status rows from Core's terminal model, which is Core's. The per-application status bar is the `status-bar` hook, which an app module answers.
- Previous and next command (Verbatim+Alt+Up and Down), which read Core's terminal model.

Built-in extensions, app modules (NVDA's `appModules/`):

- File Explorer (`explorer.py`): the shell staging windows' silence (`event_gainFocus` dropping `ForegroundStaging`, `LauncherTipWnd` and `ApplicationManager_DesktopShellWindow`, 443 to 449) as a `silent` window rule; the snap results notification (519 to 550) as an event `accept` rule, the guide's own example; good-UIA shell roots (453 to 482) as `backend` rules; the search band's MSAA focus (48 to 51) as a focus veto; tab switches announced on selection (499 to 517) as an `on-event` hook; the menu bar's exit focus dropped while a focus is pending (428 to 441) and the duplicate list-view focus (56 to 72), both hooks that need the queue's state (gap below); the notification area's focus redirect (75 to 113), a hook. The folder window row's generic list behaviour stays Core.
- The Alt+Tab switcher: NVDA's `MultitaskingViewFrameListItem` suppresses the item's containers while Alt is held, read by `getAsyncKeyState` (`explorer.py` 39 to 45), and drops the frame's focus (35 to 36). It is an Explorer behaviour, so an extension, and it is focus speech, so it must be a rule with no crossing. The rule needs Alt's state, which is today Core's stamp at arrival; the general `modifiers-held` property was already returned to Dickson (`m5-extensions-work.md`, step 2). Decision for Dickson, below.
- Notepad (`notepad.py`): tab switches, the status bar, `LineNumberBox` as an edit (a node rule).
- Calculator (`calculator.py`): history and memory item names from their children (`event_NVDAObject_init`, 52 to 59, an `on-node` hook), the name-change filter (72 to 92), `DisplayUpdated` with its own find (94 to 141), result keys that pass through (143 to 190), and typed characters always echoed (61 to 70).
- Settings (`systemsettings.py`): volume meters are not progress bars, a node rule (ADR 0003, 248, already says a built-in's rule excludes them). The Settings app row's pages, lists and search are Core's generic behaviour.
- Task Manager, Start and search, Quick Settings and Open With (`taskmgr.py`, `searchui.py`, `shellexperiencehost.py`, `openwith.py`).
- Windows Terminal's second identity: a focus veto rule of the Terminal built-in, judged in the listener (Dickson, 2026-11-09). NVDA has no Windows Terminal app module; `TermControl` is handled in `NVDAObjects/UIA/__init__.py`. The terminal behaviour stays Core and only the veto is the built-in's.
- Sleep mode stays a core per-application list with a manifest declaration (roadmap, M5), as NVDA's is an `AppModule` attribute toggled by a core command.

Built-in extensions, global modules (commands that need only the API and are not hot path):

- The system tray and taskbar lists (Verbatim+F11): NVDA's `systrayList` add-on; already ruled a global module (Dickson, 2026-10-31 and 2026-11-03).
- Speak time and date (Verbatim+F12): NVDA's `script_dateTime` (`globalCommands.py` 486); already ruled a global module (Dickson, 2026-10-31). The hot-path ruling confirms it.
- Report focus (Verbatim+Tab): NVDA's `script_reportCurrentFocus` (2956). It needs `focus()`, a live read and `speak-node`, plus spelling on a second press (gap below). It moves from Core today, a re-implementation of finished work, which the rewrite does anyway.
- Report title, not built today: NVDA's `script_title` (3210 to 3230) reads the foreground, falls back to the application's name, refuses on the lock screen, spells on a second press and copies on a third. It needs the lock state, spelling and the clipboard (gaps below).

Not classified: the event recorder (a tool).

## 2. What the extension API must offer

For each thing an NVDA app module or global plugin uses, what the settled reference has, and the gap.

- Event handlers. The reference has `on-focus`, `on-event` for five kinds (focus, selection, name change, value change, notification) and `on-node`. The inbox modules also use `typedCharacter` (Calculator), `show` (Explorer's tooltip), `UIA_itemStatus` and `valueChange` (Quick Settings), `focusEntered` (Open With) and `UIA_window_windowOpen` (Explorer). The guide already says kinds are added as modules need them, so adding these is Claude's as Verbatim's readers produce them, except where the kind is a new WIT type (item 1).
- `nextHandler` and speech after the default. NVDA's handlers speak and then call `nextHandler`, or call it and speak after. The verdicts (`pass`, `drop`, `replace`, `rewrite`) cover the first; "a hook's speech after the default announcement" is ADR 0003's open item for build B (257). Gap, Dickson's (item 1).
- The queue's state. Explorer's and Notepad's handlers read `eventHandler.isPendingEvents` and `lastQueuedFocusObject` (`explorer.py` 65, 111, 434, 506; `notepad.py` 30). Since Core calls every hook, Core can pass "a focus is pending" and "a selection of this node is pending" as arguments of the call, read when the call is sent, as NVDA reads them when the handler runs. A new argument is a WIT change. Gap, Dickson's (item 1).
- Modifier state, for the switcher: as above.
- Gestures with bindings. The reference's commands, presses and `pass-through()` cover Calculator's result keys and NVDA's `gesture.send()`. Calculator binds the digit keys (185 to 190), so every digit typed in Calculator would start a module command. Under design-v2's queue a command is an entry, and the typed echo of the next key would wait behind its host call: a host on the hot path. Proposal: a command bound with `pass-through = true` does not hold the queue; its call is ordered on its lane before any later hook of that instance, so Calculator's flag is set before the notification that reads it, and its speech is judged by the key fence on arrival. That changes queue rule 1's reach. Decision for Dickson (item 4).
- Typed reads. The reference gives one function per property and per navigation, each a separate read when not already held. Under ADR 0003 an extension has one read outstanding, and design-v2 turns each read into a `Wants` and a crossing to the outpost. A typed read lets the guest declare what it needs in one request:
  - The guest SDK takes a struct whose fields are properties from the model's property table, for example `struct Result { name: Name, automation_id: AutomationId }`, and a navigation to apply it over (the node, its children, a find), and builds one batch from the shared step list (`Read`, `Ancestors`, `Find`), which ADR 0003 already allows extensions to name at run time (52, 53). The answer is parsed at the boundary into the struct.
  - One struct read is one `Wants` and one `Resume`. Calculator's `DisplayUpdated` handler, which finds the `LandmarkTarget` and reads its children's automation ids and names, becomes one find with a children read of two fields, one crossing instead of four or more.
  - This needs a WIT function taking a property set and a navigation and returning records. It adds no kind of request. Gap, Dickson's (item 1): the WIT shape.
- Prefetch. Core sends with each hook call the event's node with the properties the tell and Core's own read already hold, and on a focus the chain, so a hook reading only those makes no `Wants`, as `extensions.md` section 4 already gives for `on-focus` ("properties() and the ancestor reads are answered from the chain"). No gap.
- Phrases with keys. `speak(message)` takes a message id from the module's strings with arguments; built-ins' strings are Verbatim's Fluent resources (`extensions.md` 2.1). For comparison with NVDA, a built-in's message ids carry NVDA's English text. Spelling (`speech.speakSpelling` in `script_title`, and Report focus's second press) and the clipboard (`api.copyToClip`, the guide's "Not settled yet") are missing. Gaps, Dickson's (item 1).
- Settings. Module settings are out of M5 by decision (roadmap 1034 to 1041). NVDA's modules read about eight settings (appendix A's count), and Calculator overrides typing echo while it runs. Typing echo is hot path, so a hook must not decide it: proposal, a module declaration `typing-echo = "always"` for its applications, applied by Core as data. Reading Verbatim's settings is a new import. Gap, Dickson's (items 1 and 5, since it pulls work into M5).
- Per-application modules and state. One instance per process on its lane, its memory its state, lost at reload or fault (reference and `extensions.md` section 3). Calculator's three fields of state fit. No gap.
- The lock state, for Report title: a read that only Core can answer, like `focus()` and `foreground()` (`extensions.md` section 14). Gap, Dickson's (item 1).
- Mouse moves (Explorer's notification area), timers, braille messages: later milestones, as the reference says.

Conflicts between the settled reference and later rulings, which the later ruling settles (Claude's to correct in the guide, not reopened):

- Who calls hooks and where reads go. The guide's "Where hooks run" says the process reading the application calls the hook and the reads go back to it; the roadmap's M5 entry and `extensions.md` section 4 say the same. ADR 0003 (Dickson, 2026-11-12) has Core call every hook and the reads go through Core, and design-v2's M5 section follows it.
- The hook's clock. `extensions.md` section 4 and the guide's Reliability page give one clock, the host's, bounded by the 11-second handling deadline. ADR 0003 gives a hook that holds the queue "a short bound counted from when Core sends the call", its figure to be set before build B. Under the hot-path ruling that bound is a hot-path figure. Decision for Dickson: the figure (proposed: set from the built-ins' measured guest time, as `extensions.md` section 7 sets the compute bound).

## 3. Cost and latency

Figures measured in this repository:

- Core to an outpost and back, for one request: 1.0 to 1.5 ms, 3.0 ms once with the outpost's worker busy, debug with the outpost logging; Core's turn is 0.20 to 0.27 ms of it; one wake each of Core and the outpost's worker (`performance.md` 155 to 164).
- D15's own reason: three pipe hops cost under a millisecond (`architecture.md` 144 and 877 to 885).
- No Wasm call, host turn or guest time has been measured. M5's step 4 was to measure the round trip and host-call overhead on both architectures (`m5-extensions-work.md`, step 4). Nothing below estimates the guest's own time.

Per handled event, counting messages under design-v2's protocol (an estimate by counting, not a measurement):

- A hook that reads nothing: Core sends `Call`, the host's lane wakes, runs the guest and answers `Final`, Core wakes. One crossing to the host and back: two wakes, one of the lane and one of Core. With the measured outpost crossing as a stand-in, about 1 to 1.5 ms on the path, plus guest time.
- Each read the hook makes: the host sends `Wants`, Core wakes, sends the batch to the outpost, the outpost's worker wakes and calls the application, Core wakes, sends `Resume`, the lane wakes. Two crossings and four wakes per read, against one crossing and two wakes under `extensions.md` section 7's design, where the outpost answered the host directly. That is the price of ADR 0003's "Core calls every hook". By the stand-in figure, about 2 to 3 ms per read, plus the application's own call.
- So a focus with one hook and one struct read adds about 3 to 4.5 ms (estimate) to a path whose budget is 10 ms and whose measured median for a live walk is 0.8 ms. A hook with three separate reads would add about 7 to 10 ms and miss the budget, which is why typed reads matter.

What must not cross:

- Every step of the hot path above. An extension changes it only by rules and declarations, which Core and the readers apply as table lookups with no crossing (the guide's rules page, "Where rules run and what they cost").
- A hook on a hot-path event, such as Calculator's `on-node` for history items or Explorer's notification area redirect, crosses only for the nodes its registration matches, so the cost falls on those controls alone. Decision for Dickson: whether built-ins may register such hooks under a measured budget, or whether each must become a rule (extending the rule actions) or Core code.

What the host must do to stay fast:

- Warm instances: one instance per process on its lane, kept between calls; compiled code pre-linked in the content-named cache; the built-in host started with Verbatim and always running (`extensions.md` 2.2 and 5). A cold compile of the built-ins takes seconds (2.2), so a cold start's first focus waits for its hook, as already ruled.
- One host per trust level, as decided: the built-ins in one host, each installed extension and the developer folder in their own (`extensions.md` section 5, X4).
- Batching on the pipe: the host writes every item of one guest run (its effects and its `Wants` or `Final`) in one pipe write, and Core takes every message waiting when it wakes, so effects add no wake. This needs no protocol change (Claude, 2026-11-28).
- Prefetch and typed reads, as in section 2.
- A ledger check from the first commit: each queue stage that awaits a host is named in the stage list, and a hot-path entry kind awaiting a host stage fails the build unless its hook is a registered module hook listed with its measurement (Claude, 2026-11-28).

## 4. How built-in extensions are verified

- Replay. The host is a simulator peer like any other: its root reducer and lanes run under the seeded scheduler, and the Wasm guest runs for real, deterministic given its imports, with time and randomness served by the simulator (design-v2, "Extensions"). Its reads are answered through Core from the world, so a different path through the module still gets answers. The invariants (no id beyond Core's held views, one answer per request, bounded calls) and the fault profiles (slow answers, a host crash, a trap, the breaker) apply.
- NVDA comparison. Unlike third-party extensions, a built-in that ports NVDA behaviour is compared with NVDA over the same world. NVDA's app modules load in the headless harness by executable name, through the replaced `getAppNameFromProcessID` (`appModuleHandler.py` 145; design-v2's M5 section). So each inbox built-in joins the comparison corpus with NVDA's own app module as its oracle, and each built-in global module that ports a `globalCommands` script (time and date, Report focus, Report title) is compared with that script. The tray list ports an add-on not in the pinned source, so it has the add-on's optional headless run or approved transcripts. This makes design-v2's "extensions are replayed but not compared with NVDA" a rule for third-party extensions only.
- Live API tests: the host calls no application, so it needs none of its own; the host's container, memory cap and wasmtime on ARM64 are tested in the host's tests and on `ci-arm64`, as M5's step 4 says.
- End-to-end: one scenario per ported behaviour asserting its captured utterance, as `extensions.md` section 13 gives.
- The dependency. Wasmtime with the component model is decided (D6, Dickson, 2026-10-22; `extensions.md` section 9), so item 6 is not reopened for the product. Two points do need him:
  - Linking wasmtime into the simulator, so built-ins run in replay tests, including on the `ci-linux` job. It is the same dependency in test tooling. Claude's, unless Dickson counts it as a new adoption.
  - Licensing: built-in guests that port NVDA's app modules are GPL. The guest SDK is permissive (`m5-extensions-work.md`, step 5), `verbatim-ext-api` is in the platform-neutral tier (CLAUDE.md), and the guest workspace is outside the tier check. Decision for Dickson (item 6): built-in guests in a GPL guest tier, free to port NVDA's app modules, with the SDK and the API crate kept original.

## 5. What changes in design.md if adopted

- Terms and peers: the built-in host is part of the skeleton from the first build that needs a hook, not "from M5"; extension hosts are listed as peers without the milestone.
- Tiers (design-v2 888): Core moves from platform-neutral to GPL tier (Dickson, 2026-11-28); a guest tier is added for built-in modules, pending his licensing decision.
- Records, Core: an `Extensions` record holding each extension's lifecycle as one state value (`Loading`, `RulesReady`, `Active`, `Failed`, `Disabled`, as `extensions.md` 2.2 tables it), the rule tables keyed by identity, the hook registrations and the gesture map's version. Each `Application` gains its instances' lane records: the ids each instance holds, which the held view names, and its call in flight. The list-dialog records already there stay.
- Rules: design-v2's placement (the listener's part in the `Watch` filters, every other rule applied in Core to answers) replaces `extensions.md` section 4's placement of node rules in the outpost's worker. Rules are in the focus slice, since the Explorer world's staging windows and shell roots need them; the slice's Explorer behaviour is written as the Explorer built-in's rules, not as Core constants.
- Queue: the stages `ext.call` and `ext.read` with their end causes; the pass-through command that does not hold the queue, if Dickson approves; hook-call arguments carrying the prefetched node and Core's queue facts.
- The hot path: a section defining it as above, with the ledger check, beside "Checks from the first commit".
- Adding a feature: two forms. A Core feature as today; an application's or a global command's feature as a module folder: `module.toml` with its rules, hooks and commands, the guest crate if it has code, its strings, a world with NVDA's app module or script as its oracle, and its parity entry.
- Verification, "Extensions": built-ins compared with NVDA, third-party extensions replayed only.
- M5 section: the costs above in place of "one read outstanding per extension, as ruled" alone; the typed read; the conflicts with the guide resolved in ADR 0003's favour.
- Sizes: the host, the API crate and the guest SDK are not estimated here; the slice's Explorer constants move into module data, which changes no line count materially.
- Build order: rules and the extension lifecycle land with the readers (step 5); the host, with one built-in hook and the measurement of the crossing and guest time against D15, comes before any built-in with a hook is ported.
- All decisions for Dickson: the items below.

## Decisions for Dickson

1. The hot path: confirm the five steps above and that the synthesizer stays a built-in extension on it, as ruled 2026-11-09.
2. Review cursor, object navigation and say-all commands: Core (recommended, because each moves Core's own state, and an extension would need write imports into the navigator and review position, a new contract), or a built-in global module with those imports.
3. Hooks on the hot path for built-ins: allowed when narrowly registered and measured against D15 (recommended), or rules and Core only.
4. The switcher's Alt state as a `modifiers-held` matcher property (already his, `m5-extensions-work.md` step 2).
5. A pass-through command that does not hold the queue (item 4), for Calculator's digit keys.
6. The hot-path hook bound's figure (ADR 0003's open bound).
7. The WIT additions (item 1): typed reads, the queue-state and lock-state reads, spelling, the clipboard, speech after the default, the new event kinds that need new types.
8. Settings for modules: the `typing-echo` declaration, and reading Verbatim's settings, pulled into M5's scope (item 5).
9. Licensing (item 6): built-in guests as a GPL guest tier.
