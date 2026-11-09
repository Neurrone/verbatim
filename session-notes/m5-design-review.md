# Adversarial review: the restated M5 extensions design

Reviewed on 2026-11-09, branch claude/charming-noether-iqtlei: `docs/design/extensions.md`, `docs/design/m5-extensions-work.md`, the Extensions guide's reference and concepts page, `session-notes/m5-decisions-in-order.md`, against the architecture (D6, D9, D13, D14, section 1), the security model (X4, X5, X6, A9, C9), the ownership model and `docs/design/ownership-milestone-rules.md`, and the code of `verbatim-outpost`, `verbatim-outpost-logic`, `verbatim-uia` and `verbatim-input`.

Terms used below: a "lane" is one thread in a host (one per outpost served, one per global module); "the hook wait" is the outpost worker waiting for a host's answer to a hook; "the host pipe" is the supervisor-made pipe between one host and one outpost.

Counts: 6 high, 12 medium, 5 low. Every fix keeps Dickson's 2026-11-09 decisions as given in the brief.

## High

### H1. The hook wait nested in the worker has no owner for the host pipe, and its reads have no answer path on abandonment (CONFIRMED)

The design (section 4, "Hooks are called from the outpost's worker, which waits for the host's answer within the event's deadline and serves the host's reads meanwhile"; step 0b, "a waiting state counted with the worker's others") puts a cross-process wait and a nested request loop inside a worker program. The outpost today (`verbatim-outpost-logic/src/lib.rs` header; `verbatim-outpost/src/outpost/worker.rs` header) runs one program per worker generation, the reducer holding "the program in flight with the worker generation in charge, its deadline and its moved-on rule ... the queries not yet answered", and abandons a program "at its deadline or once the user moved on" from its window.

Sequence:

1. Explorer's outpost reducer dispatches a focus program to worker generation 4. The program reads the focus (naming its window in the charge), applies node rules, and calls the built-in `on-focus` on lane L of the built-in host, then waits on the host pipe.
2. The guest calls `find-descendant`; the host sends read R1 on the host pipe. Worker 4, in its wait loop, reads R1 and starts the UIA call.
3. The user presses Alt+Tab. The shell's half-second check gives `Judged { moved_on: true }`; the reducer abandons generation 4, answers the program's query "abandoned", and starts generation 5. R1 is not in the reducer's table of unanswered queries, so nothing answers it.
4. Generation 5 takes the next entry, a focus in another Explorer window, and calls `on-focus` on lane L. Lane L is still inside the first hook, blocked on R1, so the call queues in the host.
5. Generation 4's UIA call returns. Its gate refuses further counted calls, but it is still the thread in its wait loop reading the host pipe. Generation 5 is now also reading the same pipe for its own hook's answer. Two threads read one pipe: a message is taken by whichever reads first, so generation 4 may consume the answer to generation 5's hook, or generation 5 may receive R1's follow-up read and run it under its own event's deadline.

What goes wrong: a request (R1) with no answer path; two readers of one pipe, which is hand-written concurrency the design principles forbid; an abandoned worker that is not hung (it is waiting on our own host, bounded by the epoch) counted towards the eight abandoned workers that kill the outpost. A busy module with an author who moves quickly through Explorer can therefore get Explorer's outpost killed and replaced. It also contradicts the milestone rule "A hook runs on a worker in a call program, and its answer is a reducer input", and that file's "The wasmtime store is owned by the worker generation in charge" is stale under the host design.

Smallest fix: the hook wait is a state of the outpost's reducer, not of a worker. The host pipe is read by the outpost's shell (the I/O thread's completion port, as Core's pipe is), and every host message is a reducer input carrying a hook-call id. A pending hook is one reducer state value (`AwaitingHook { lane, call, layer, deadline }`) that holds the intake as a marker does; each read is dispatched to the worker in charge as an ordinary program tagged with the call id, so abandonment answers it ("deadline") through the existing one-answer rule, and the moved-on rule does not apply while no program is in flight. The chain's resolution between layers is then the reducer's pure function, as step 0b wants, with no worker holding chain state.

### H2. A click from a hook deadlocks against the outpost waiting on the same lane (CONFIRMED)

Section 4: "a click answered by asking the node's outpost where the element is now and then moving the pointer and clicking from a shell thread"; "every `on-list-dialog` [goes] from the reducer straight to the host"; an app module's `on-command` runs "through the outpost". A lane's calls are serial.

Sequence (an app module that shows a list dialog and clicks the chosen item, as a ported Poedit-style or tray-style module would):

1. The user picks an item and presses Enter in the module's list dialog. The GUI thread reports the answer; Core's reducer sends `on-list-dialog` to lane L (the lane for application A's outpost) of the module's host.
2. The dialog closes and focus returns to A. The listener reads the focus; A's outpost worker calls the module's `on-focus` on lane L. Lane L is busy with `on-list-dialog`, so `on-focus` queues.
3. `on-list-dialog` calls `click(node, left)`. The host sends the click to Core; Core asks A's outpost where the element is.
4. A's outpost is in the hook wait for `on-focus` (step 2) and serves only host reads, so Core's query queues behind the hook.
5. Cycle: the click waits for Core's query, which waits for `on-focus`, which waits for lane L, which is inside `on-list-dialog` waiting for the click. It breaks only when the event's 10-second deadline expires: ten seconds of silence after every such answer, and a failed click.

The same cycle arises with an app module's `on-command` that clicks (run through the outpost, which is waiting on that very hook).

Smallest fix: the click's location is a read, sent by the host to the node's outpost on the host pipe (a hidden `node.place()` answered by the outpost's readers), and the host sends Core a click with the rectangle already read; Core only moves the pointer and clicks from the shell thread. Core then never queries an outpost on a module's behalf during a hook. With H1's fix, an outpost waiting on a hook also stays free to answer Core's queries.

### H3. A module's speech and the event it was spoken for reach Core by two routes with no order (CONFIRMED)

Section 4 sends "speech judged by attention" from the host to the reducer, while the hook's verdict goes from the host to the outpost, whose report goes to Core. The ownership model's problem statement names exactly this: "a decision waited on a message from another process arriving first".

Sequence:

1. Focus moves in Notepad. Notepad's outpost calls an installed extension's `on-focus`. The hook calls `speak("Line 3")` and returns `pass`.
2. The host writes the speech to its Core pipe and the `pass` to the outpost pipe.
3. Two outcomes, decided by thread scheduling: either Core receives the speech first and queues it, then the outpost's focus report arrives and its announcement cancels current speech (a focus interrupts), so "Line 3" is lost or cut; or the report arrives first and "Line 3" is queued after the focus announcement.

NVDA's order is deterministic: `eventHandler` cancels speech before the gain-focus chain runs, and the app module's message is spoken before `nextHandler()` speaks the focus. The same race exists for `cancel-speech`, `speak-node` and `tone` called from `on-focus`, `on-event` and `on-node`.

Smallest fix: effects made during a hook the outpost called travel back with the verdict on the host pipe, and the outpost forwards them to Core inside the event's report, in call order; Core still judges them by attention, so the decision that speech goes "through Core" holds. Effects from hooks Core called directly (a global module's `on-command`, every `on-list-dialog`) keep the host-to-Core route, which has only one sender.

### H4. A focus veto placed only in the listener does not apply to focuses the outpost reads itself (CONFIRMED)

Section 4: "a focus veto naming only what the listener's event shape caches runs in the listener before its read, and otherwise in the outpost". So a listener-placed veto is absent from the outpost. Yet the outpost reads focuses itself: after a menu or the Alt+Tab switcher ends, "Core ... asks the foreground application for its focus if its own has not changed shortly after"; after the listener is replaced, "Core reads the foreground window itself and asks that application's outpost for its current focus"; a replacement outpost is asked for the current focus.

Sequence: Windows Terminal is in front with its focus on the `TermControl` whose runtime id starts 42. The user opens and closes the system menu (Alt+Space, Escape). No new focus arrives in time, so Core asks Terminal's outpost for its focus. UIA's focused element is the twin whose runtime id starts 14. The outpost has no veto (it was placed in the listener), so it reports it, and "terminal" is spoken again: the exact utterance Dickson's step 9 decision removes.

The milestone rules already state the needed rule ("the outpost's rules apply to recovery reads under its current generation"), but the design and step 2 place each veto in one process only.

Smallest fix: the loader places a listener-eligible veto in both the listener (before its read) and the outpost (on every focus the outpost reads itself), and step 2's placement tests and step 9's tests include the menu-end recovery read.

### H5. Until the first publication, nothing applies the built-ins' rules, and that publication waits on compiling every component (CONFIRMED)

Step 3 deletes the Explorer constants ("the Explorer scenarios stay green with the constants deleted"). Section 2.2: the loader "reports a finished result ... validated and ready at a version ... never a step in between", and "ready" includes the host having compiled and pre-linked the components. Decision 12: the built-in host compiles built-ins at load into the cache beside a portable copy's executable. The listener's `Ready` gates input; nothing gates it on the first table publication.

Sequence: a portable copy starts with Windows Terminal or Explorer in front, and the cache is empty (first run, or after an update). The listener says `Ready` and Core announces the foreground's focus. The loader is still waiting for the built-in host to compile every built-in component with Cranelift, which can take seconds on a cold cache. The focus is judged with no tables: no shell-root `backend = "uia"` rule, no Terminal veto. So the duplicate "terminal" or the wrong backend's announcement is spoken, and anything else in that window is read the old way too. The same holds on every developer-mode start and after any built-in failed to compile.

Smallest fix: the loader reports an extension's rules as soon as they are validated and compiled into tables, separately from its components' readiness (two results, rules then code, each final), and Core holds the listener's `Ready`, and so input, until the built-in extension's tables are published. Hooks registered by a component not yet ready are skipped as `pass` for that window, which is stated, logged and limited to start-up.

### H6. The multi-press hold shows the first dialog when the first command is slow, and is undefined across modules (CONFIRMED)

Section 4: a dialog a command opens is `Held` until its press's multi-press window closes "when the same keys are bound to a higher count", so "a double press never shows the first dialog" (Dickson, 2026-11-09). A global module's lane is serial.

Sequence A, a slow first command:

1. Verbatim+F11, press 1 at t0. The hook thread stamps a window ending at t0+500 ms. The reducer sends `on-command(tray-list)` to the global lane. The command calls `find-window` on Explorer, which is slow, about 700 ms.
2. Press 2 at t0+300 ms. The reducer sends `on-command(taskbar-list)`, which queues on the lane behind press 1's.
3. At t0+700 ms, press 1's `show-list-dialog` reaches Core. Its window has closed, so it is shown `Open` at once and the tray list is spoken.
4. Press 2's command then runs and replaces the dialog. The user heard the first dialog, which the decision rules out.

Sequence B, two modules: an installed extension E binds Verbatim+F11 to one press (its gesture conflict was listed at install), and the built-in tray module still binds two presses. Press 1 runs E's command, whose dialog is `Held` because a higher count is bound. Press 2 runs the built-in's command, whose dialog opens. "A dialog from the higher-count command replaces it" cannot hold: one dialog per module, and the two are different modules. E's held dialog has no stated end, and E's `on-list-dialog` has no stated answer.

Sequence C, a map swap: press 1 is raised under gesture map version 7, where two presses are bound. A reload publishes version 8, which drops the two-press binding. The reducer judges "bound to a higher count" against its own map, version 8, while the hook thread's stamp came from version 7. Two owners judge one fact.

Smallest fix: the hook thread alone decides whether a higher count is bound, from the snapshot it resolved the press with, and stamps the gesture with `hold_until` only then. The reducer's `Held` state ends at the later of that time and the end of every later press of the same keys already raised, so press 1's dialog stays held until press 2's command has answered. A `Held` dialog superseded by a later press's command in another module is withdrawn, never shown, and its module receives `on-list-dialog(cancelled)`, which is its one answer. Dickson may prefer a different answer for case B, since it is what users hear.

## Medium

### M1. A new outpost's first events race the host's lane, the pipe and the tables (CONFIRMED gap)

X4 starts an installed extension's host "while one of its applications has an outpost"; the supervisor makes the host pipe; spare outposts get their application at assignment. Nothing orders these. Sequence: Notepad gains focus; the supervisor assigns a spare outpost and releases the held facts. That outpost has not yet received the current tables, and E's host has not started or has not created lane and instance. Either the first focus is judged with no rules and no hooks, so the module's rewrite or drop is missing on exactly the focus users hear first, or the worker calls a hook with no pipe, which has no stated answer. Fix: the assignment carries the current table version. A hook registration is enabled in an outpost only once the supervisor reports the lane ready. The design states the first-focus rule: wait for the lane, bounded by the host's measured start-up time, or pass. The reverse order for unloading, outposts first and then the lane, already follows from "the host drops an old instance once no outpost still calls it".

### M2. Module speech is not fenced by the key that caused it (CONFIRMED gap)

The guide promises "a second press interrupts the first one's speech, as with the time and date". Sequence: Verbatim+F12 twice within 500 ms. Press 2 cancels speech in the reducer as a gesture does, then press 1's `on-command` answer arrives from the global lane with the time; press 2's date is queued behind it. The user hears time then date, not date alone. `speak` is "queued behind current speech", and a global module's `cancel-speech` has no attention application to be judged against. Fix: every effect from an `on-command` carries the key number of its press (Core mints it, and the call carries it), and the existing fence on expired gesture speech drops speech of a press superseded by a later key. State what `cancel-speech` means for a global module.

### M3. Developer-folder modules have no host, breaker unit or order (CONFIRMED gap)

The developer folder holds "unpacked module folders", which have no extension manifest, id or `locale/`. Section 1 defines a host as one extension's process; A9's breaker is per extension for non-built-ins; the security model's 2026-11-06 entry says "developer-folder modules in hosts of their own (X5)", which the restated X5 no longer says; the concepts page says they "run as installed extensions do". Also, "two developer-folder modules claiming one application fail the later" has no order at start-up, when the loader reports each result independently, so which one fails varies between runs. The same applies to two installed extensions claiming one application found at load (a folder copied by hand). Install refuses that case, but load only "marks failed" with no rule for which. Fix: each developer module runs in a host of its own with its folder path as its id, a breaker per module, and its strings from its own `locale/`. Conflicts at load are decided by the reducer from the whole set, deterministically (both fail, or the earlier id wins), never by arrival order.

### M4. The built-in host's death and memory cap are attributed to the wrong modules (CONFIRMED)

A9 counts "the host ending while one of the extension's calls was in flight" as a failure against each extension with a call in flight, and for built-ins counts per module. Sequence: a host-side defect or the process memory cap ends the built-in host three times in a minute. Each time, Explorer's and Calculator's modules happen to have hooks in flight, because they are the busiest. Both are turned off for the session, though neither caused it, while the module at fault, say a leaking global module, is never counted. The process cap is also shared by one instance per outpost per module plus the global lanes, so legitimate use grows with the number of open applications and can reach a fixed cap. Fix: a host death counts against a built-in module only when the death is attributable to it (a store trap or limit hit by that module's instance); otherwise it counts against the host, which is restarted with the outposts' backoff. The built-in host's cap scales with the lane count, or is set from a measurement with many outposts open. State that the always-running host is restarted, and that every outpost treats an in-flight hook on a broken pipe as `pass`.

### M5. Two resolvers of application identity, and a listener cache keyed by a reusable process id (PLAUSIBLE)

Section 3: the supervisor resolves identity at launch, and the listener separately "with one cached image-name query per process id". The milestone rule says "Application identity is one function of the held process", and the design principles say state about an owner lives in its record. Sequence: a process exits and Windows reuses its id for a different executable. The listener has no outpost record for it (window rules and event drops apply "before any outpost exists"), so its cache still names the old executable, and it applies another application's window rules, event drops and vetoes. For `msedgewebview2.exe`, the parent process may have exited between the listener's lookup and the supervisor's, so the two disagree on identity and the listener's veto and the outpost's rules come from different modules. Fix: one resolver. The listener resolves identity once, from a process handle it holds until the process exits, and sends it with the first fact for that process; the supervisor uses that identity rather than resolving its own. Cache entries are keyed by handle or creation time and dropped at process exit.

### M6. The version rule is stated but its meaning is not (CONFIRMED)

1. "Every event is judged under one version" holds within one outpost, but one focus is judged by the listener (veto), the outpost (node rules, hooks) and Core (`silent`, `accept`), each of which swaps at its own time. A focus can pass the listener under version 7 and get hooks under version 8.
2. The reducer keeps "one state value per extension (`Active` at a version)", but what is published is the merged table set of every layer. The design needs two named facts: each extension's version, and the generation of the merged publication that judgements name.
3. "Core recognises a judgement made under a version it has moved past" says nothing of what Core then does: for a veto, a dropped event cannot be re-read; for speech from a disabled module's hook, drop it or speak it?
4. A built-in disabled by the breaker is per module, which "one state value per extension" cannot represent.
5. "The host drops an old instance once no outpost still calls it" needs either outposts' acknowledgements or the lane's rule.

Fix: a merged generation is the published fact. For each fact kind, a stale judgement has one stated action: veto drops stand; module effects from a version whose extension is now `Disabled` or `Failed` are dropped; everything else is used. The built-in extension's state carries a set of disabled modules. A lane drops its old instance at the first call naming a newer version, which needs no acknowledgement because a lane serves one outpost.

### M7. `normalize-class` runs in the outpost, but listener-placed rules match the normalized class (CONFIRMED)

matchers.md: `window-class` is "its window's class, after any `normalize-class` rule". Section 4 places `normalize-class` "where a reader first reads the window", and focus vetoes naming only `ListenerEvent` properties, `window-class` among them, in the listener. Unless the listener also has the normalize table, a listener veto or event drop on a normalized class never matches, while the same rule placed in the outpost does. So a rule's effect depends on where the loader put it. Fix: `normalize-class` rules are placed in every reader that evaluates `window-class`, the listener included, and the placement tests cover it.

### M8. Hook-chain semantics contradict each other (CONFIRMED)

The hooks reference: "`pass` hands the event to the next; any other verdict ends the chain." The concepts page: hooks run "each seeing the control as the layers above left it, until one ends the chain", which implies that `rewrite` continues. Section 6: "a drop never undone", which only means something if the chain goes on past a drop. Sequence: an installed extension's `on-focus` returns `rewrite(name)` for a Notepad control that the built-in's `on-focus` would `drop`. By the reference, the built-in never runs and the control is spoken. By the concepts page, the built-in runs and drops it. The order for `on-node` across layers, and whether `on-node` or `on-focus` runs first on a focus batch, are unstated. Fix: a decision, Dickson's because users hear it. The likely answer is that `rewrite` and `on-node` changes continue down the chain, and `drop` and `replace` end it. The three documents should then be made to agree, with the pure chain function's tests pinning it.

### M9. Global and app-module commands have no stated precedence, and the app-module route judges the foreground twice (CONFIRMED gap; NVDA cited)

"For commands, the highest layer that binds a gesture gets it" orders only the layers of one application. A global module is in no application's layers. NVDA looks up global plugins before the focus's app module (`nvda/source/scriptHandler.py`, lines 183 to 185). Separately, an app module's command is active "while a claimed application is in front". The hook thread decides whether to swallow from its copy of the foreground, but the reducer routes `on-command` to an outpost from its attention record. Sequence: the user switches from Poedit to Notepad and presses Alt+1 at once. The hook thread, not yet told of the switch, swallows Alt+1 for Poedit's module. The reducer, which has moved attention, routes the command to Notepad's outpost, which has no such command, so it is lost, or reaches the wrong module. Fix: state the order, global modules before app modules as NVDA has them unless Dickson decides otherwise. The raised gesture names the application whose binding the hook resolved, and the reducer routes to that application's outpost.

### M10. A lane's dialog and held nodes across host death, reload, breaker and outpost replacement (CONFIRMED gap; listed in step 0b without the cases)

1. Reload: lane G swaps to version 2 while its version-1 instance's dialog is `Open`. The user answers, and `on-list-dialog(id)` reaches an instance that never issued that id and holds none of its nodes.
2. Host death while `Open`: the dialog stays on screen, and its answer goes to a dead lane.
3. The breaker disables the module while `Held`: the alarm fires for a disabled module.
4. Outpost kill-and-replace: an app module's lane is per outpost, so its instance, state and dialog are lost although the application process is unchanged, unlike NVDA's one `AppModule` per process.

Fix: the dialog record lives in the lane's record keyed by instance, and the instance's end closes the dialog with no answer: on swap, host death, breaker, or the outpost's end. Consider keying an app module's instance by application process rather than outpost generation, with only node handles tied to the generation. Pin each case in the generated test's world.

### M11. Global reads need routes the design does not give (CONFIRMED gap)

1. `find-window("explorer.exe", ...)` with Explorer's "separate process for folder windows" option: two `explorer.exe` processes, two outposts. Who enumerates windows to pick the process? The host is in an AppContainer, and reads go to "the claiming outpost".
2. "When it reads an application with no outpost, the supervisor starts one": the host has no pipe yet and no route to ask the supervisor. The read's answer while the start is in backoff (up to 30 seconds) is unstated.
3. An outpost may be retired when idle while a global module holds its nodes, such as the tray list's items while its dialog is open, so the click returns `gone`.
4. A Core-called hook has no event, so its reads' deadline is unstated.
5. While an outpost waits on one lane's hook, it is unstated whether it serves another lane's reads, which then spend that event's deadline, or holds them, which delays them up to 10 seconds.

Fix: Core resolves `find-window`'s first class to a window and process from the window list it already reads hang-safely, asks the supervisor for that process's outpost and pipe, and answers the host. Nodes held by a host count against idle retirement. `on-command` and `on-list-dialog` get a stated deadline. A waiting outpost serves only its own hook's reads; others queue as programs.

### M12. `on-focus` needs the outpost's own element before the report, moving the re-find onto the focus path (PLAUSIBLE)

Today the outpost "reports the focus from the read at once ... and only then finds its own copy of the element by the chain's runtime ids". A hook's reads (`parent()`, `find-descendant`) need that element, and the report must wait for the hook, so for every hooked application (Explorer, Calculator) the re-find moves before the report on every focus. Fix: measure it in step 4 against D15. Let the listener's read answer `properties()` and ancestor reads from the chain it already carries, and find the element lazily on the first read that needs a live call.

## Low

### L1. `speak-node`'s node can be released before Core fetches it (PLAUSIBLE)

The host reports its held nodes to the outpost; Core learns the node from the effect by another route. If announcing it as a focus needs a fetch (position, ancestors), the host's next held report can release it first, and the announcement says nothing. Fix: the effect carries the snapshot the host already holds, or the release rule counts a node named in an effect in flight.

### L2. `focus()` has two definitions (CONFIRMED)

imports.md: "none when it is in an application you cannot read"; the milestone rule: "the outpost's newest report of a control". For an app module handling a background event, these differ. For a global module, `focus()` is Core's fact, with no route to the host. Pick one and give global modules a Core query.

### L3. Compiling at install needs a host X4 says is not running (CONFIRMED)

The loader "asks the extension's host to compile", while X4 starts an installed extension's host only while one of its applications has an outpost. The host's death mid-compile needs a stated answer for the loader. Fix: the supervisor starts the host for the compile and ends it after; a pipe break answers the loader "failed: host ended".

### L4. `on-node` on every emission (PLAUSIBLE)

`on-node` changes "apply wherever the control goes", but the hook is called "once per emitted batch", so a node re-emitted on each property change costs a round trip each time, and a broad registration under an event flood limits the worker to the round-trip rate. Fix: cache the changes per held node until it is released or its matched properties change, and measure.

### L5. Document inconsistencies (CONFIRMED)

1. concepts.md says the "two speech engines are two more" extensions, while extensions.md section 12 and the decision log keep that open.
2. extensions.md section 5 points to "section 13" for the engines' question, which is in section 12, and the decision log cites a "section 13.3" that no longer exists.
3. limits.md and section 7 say ten seconds per event; `verbatim_model::HANDLING_DEADLINE` is 11 seconds (transaction timeout plus a margin).
4. `docs/design/ownership-milestone-rules.md`'s M5 entries ("the wasmtime store is owned by the worker generation in charge", "a listener veto names its generation" with no outpost copy) describe the superseded in-outpost runtime.
5. Section 2.2 says the loader's results are the reducer's "only inputs" to the lifecycle, but user disable, the breaker and host death are inputs too.
