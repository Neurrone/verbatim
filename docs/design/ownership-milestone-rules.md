# The ownership rules for the planned milestones

The ownership refactor's builds are all built: [the one judge build](../../books/dev/src/explanation/design/ownership.md), [the input build](../../books/dev/src/explanation/design/ownership.md), [the messages build](../../books/dev/src/explanation/design/ownership.md), [the output build](../../books/dev/src/explanation/design/ownership.md) and [the outpost's reducer build](../../books/dev/src/explanation/design/ownership.md), under [the ownership model](../../books/dev/src/explanation/design/ownership.md). What remains of the plan is the first list below: the rules each planned milestone follows under the model, to be moved into the roadmap's milestone sections as each is designed; the second is what each must keep of ADR 0003's model. The evidence list names the commits behind each rule; the study, the audits and the reviews are in `experiments/ownership-study/`.

## Rules

- M4.3.
  - Each setting has one owner and reaches it as a delta, and every setting writes through the saver.
  - The hook's settings and gesture map are one snapshot, loaded once per key.
  - Cancelling expired focus speech is Core leaving out `DropExpiredSpeech`.
- M5.
  - The loader owns the rule tables, configuration with a generation.
  - A listener veto names its generation, and is placed in the outpost too, whose rules apply to its own recovery reads under its current generation.
  - A module's `focus()` is the outpost's newest report of a control, its own fact; a module that reacts to changes is given every change of a node the outpost has not released, and Core decides what is spoken.
  - A pending hook is one state of the outpost's reducer, which holds its event intake meanwhile; every message from the host is a reducer input naming its hook call, and each read is a program named by its call.
  - The wasmtime store is owned by its lane in the extension's host; a trap or overrun discards the store and instantiates the module again, and a hook abandoned at the event's deadline counts as `pass`.
  - Load, swap and `on-command` are queries with responders.
  - A module's speech is a report judged against attention.
  - Module commands enter by the one input route.
  - Sleep mode is a view keyed by the foreground window.
  - Application identity is one function of the held process.
  - The application rules move into modules once.
- M6.
  - One owner per host, with users holding leases.
  - A non-active synthesizer's voices are a query.
  - Adapter callbacks only write frames.
  - The host's owed state is a typestate.
- M6.1: MSAA and IA2 only, since UIA cannot be cancelled.
  - The worker calls `CoEnableCallCancellation` at its start.
  - The worker publishes its current call ticket under a ticket lock as it enters and leaves each call.
  - The canceller holds that lock across `CoGetCancelObject` and `ICancelMethodCalls::Cancel`, and does nothing for a ticket no longer current.
- M6.2: starting an outpost generation takes proof that the previous one exited; a marker's outcome names its generation.
- M6.3.
  - The queue thread owns each utterance's ending, and the mixer only each attempt's.
  - `Source::register` consumes an admission token (3ebb7be0).
  - The awake period is configuration, under the real-time rule.
- M6.4: replies name the start they read from; an owed read has a timer input.
- M6.5: the classic edit control's drop of a caret event equal to the last read is a skip on the reader's own record, which is allowed.
- M7.
  - The helper talks only to its outpost.
  - A buffer in the helper sends numbered one-way updates.
  - Calls to the helper can be cancelled with `CancelIoEx`.
  - The helper's UI-thread work is thread-affine.
  - The store keys IA2 nodes by window and unique id.
- M8.3.
  - Core decides the active profile.
  - A switch is one transition whose effects carry its generation, with the theme and synthesizer settings ordered before the `Speak` the trigger caused.
  - Triggers are suspended while the settings dialog is focused.
  - Overlays are written only by reducer effects.
  - An OCR read is a query.
  - Words stay in Core.
- M8.7: two instances share no writable fact; the integrity of the window in front stays the listener's report.
- M9: logs are one-way and sent in batches; the Verbatim+F1 report is a query.
- M10: the console reads Core only by queries; its node reads are requests with responders.
- M11: a theme switch is an input and an effect ordered with `Speak`.
- M12: remote input enters the one route; the protocol is versioned and validated.
- M16: Java objects are released by the release rule; establish first whether the bridge is thread-affine.
- M17: drawing threads only append; one helper thread owns the model, sending numbered snapshots that Core judges.
- M18.
  - Core alone decides content and tethering.
  - Braille keys enter the one route with the buffer generation and a key number.
  - Speech and braille stay in step through the existing marks.

## What the later milestones keep of "Readers tell, Core asks"

What each later milestone must not lose of ADR 0003's model (`books/dev/src/reference/adr/0003-readers-tell-core-asks.md`), with NVDA's source for each; M5's are in `docs/design/m5-build-a.md`, section 20.

M4.3, M4.7, M4.8:

- M4.3's spelling check (NVDA's 50 ms after a word ends, `NVDAObjects/behaviors.py` 260 to 313, not verified) is a request outside the event queue, held by the outpost in its waiting table; it reads later than NVDA's.
- M4.7's bypass (pass keys after a 1-second reducer step, `reducer_busy`, not built) is still needed: the 150 ms release runs on the reducer thread. M4.7's liveness from the working threads is what sees a stalled listener WinEvent thread, which pongs cannot. M4.8 releases held keys on a panic from Core's stroke line, the one line of held keys. After a Core restart the guardian holds launches up to its 13-second grace (`docs/design/guardian.md` 113 to 119, not verified); a first focus read waiting for its outpost then stops waiting silently.

M7, the in-process helper and browse mode:

- The helper is part of its outpost: it talks only to that outpost, which hands it its channel and treats what it sends as untrusted (security mitigation X8, `docs/design/security-model.md` 206; trust list 40; `docs/design/ownership-milestone-rules.md` 47 to 51). It is in every application with a text field, building and repairing the browse buffer inside a browser and reading live-region text at the event (D2 amended).
- Its presence is a fact of each application, ending with it: absent in a process of a bitness with no helper, a console application, a higher-integrity process, an AppContainer or suspendable process for IAccessible2, or where loading failed or its version was refused (`nvdaHelper/remote/injection.cpp` 318 to 321; `IA2Support.cpp` 147 to 173). A request it would serve is answered another way or "unavailable"; nothing waits for a helper tell.
- Its held objects carry a generation of its own, ending when its channel closes or its process exits, not at an outpost's replacement. Its posted work cannot be withdrawn (`nvdaHelper/remote/inProcess.cpp` 171 to 206), so a stop drops only the answer, and the outpost's request ends at Core's due time like any other.
- One teller per event kind: the outpost tells only the kinds M7 assigns to its helper, none the listener tells, though the helper's in-process hook observes every event of its process (`injection.cpp` 159).
- Questions for Dickson: how the helper reaches every application without the global hook X8 forbids (NVDA loads it by one, `injection.cpp` 322 to 333), a trust boundary; how long an application may wait on Verbatim at loading or at a tell (NVDA's thread waits up to 10 seconds at its first focus, `injection.cpp` 274 to 293, `rpcSrv.cpp` 80), a budget; the helper in verbatim.exe's own process for composed text in its dialogs (`IA2Support.cpp` 148 to 154), a trust boundary.
- Also M7's: the IAccessible2 proxy registration in each UI thread (`IA2Support.cpp` 49 to 73); the channel's transport and ending; a buffer surviving a channel's replacement; the helper's cost to every application, judged against the event-path budget; "numbered one-way updates" against the "changed, told once until asked" tell.
- Composed text: which source echoes typed characters (`keyboardHandler.py` 178 to 202); whether the helper's composition tells are synchronous (`ime.cpp` 418, 432); a password flag on a focus tell; the composition as a Core-owned focus updated at the tell (`NVDAHelper/__init__.py` 419 to 443) and caret keys inside it; candidate windows (`eventHandler.py` 506 to 515; `NVDAObjects/IAccessible/mscandui.py` 78 to 80, 167 to 188, 281 to 302), which the MSAA change rule drops today; the helper's own hooks (`tsf.cpp` 568 to 604). The helper's tells from `nvdaControllerInternal.idl`: a typed character, a composition update (starting one), a composition's end with its text, a candidate list update and close, open status, conversion mode, an input language or layout change. Until M7 compositions and candidates stay silent and typed characters come only from the hook (`typed.rs` 30 to 36, 151 to 153).
- Browse mode's page: a buffer repaired from outside has no teller for a page's text changes (NVDA hears no text insertion from outside), and IAccessible2 live-region added text is readable only inside the event (`textFromIAccessible.h` 23 to 25), hence the helper. Still open for Dickson with M7: where the reading position lives (in Core by path, or in the helper's buffer as NVDA keeps it); whether a helper's buffer is read on a second lane beside the one worker, without which it is no better than NVDA on a hung page; what a key does when pressed during a slow focus inside a document (NVDA chooses the action at the press; a letter typed just after Tab into an edit field reaches the field only if pressed after the focus executed; needs a capture); how the movement's answer and a focus judged meanwhile are ordered; backed-up movement keys (NVDA discards all but the last waiting caret or table move, `cursorManager.py` 253 to 256); a lost position; say-all's read-ahead depth and its behaviour at "not responding". The published view must carry the focused document's mode, single-letter navigation and trapping. The caret watch's bound is a parameter (Word's Tab waits up to a second). UI Automation documents (Word, Outlook's viewer) have no buffer NVDA can keep current; held ranges as anchors. The roadmap's preference for no page buffer (Dickson, 2026-11-10) now conflicts with D2 as amended and needs reconciling. Generic mechanisms M7 needs: self-contained positions (node, ancestors, offset), long requests continued in pieces, a newest-wins request, the IAccessible2 caret and attribute events (not hooked).
- The roadmap's M7 echo item says the helper echoes composed text "as NVDA echoes what the application receives"; NVDA speaks the composition's inserted and committed text and suppresses character messages after a composition's end (`ime.cpp` 496 to 501; `tsf.cpp` 490 to 499).

M8.3, profiles:

- The profile stack is Core's fact; the settings a read needs travel in its request, never pushed to outposts. The switch is a stage of the executed focus entry in NVDA's place: the old focus's lose-focus (`api.py` 93 to 94), `handleAppSwitch` (183): the switch notification (`appModuleHandler.py` 397 to 398), leaving modules' lose-focus (400 to 416), one atomic exit and enter of triggers (430 to 446), arriving modules' gain-focus (454 to 457); then the commit (`api.py` 185 to 187). Entries behind a pending focus are judged under the old stack; a focus Core stops waiting for, or refused, leaves it unchanged. The modules compared are those of the whole old and new focus, each with its ancestors (`api.py` 102, 150 to 154), so a focus answer must be able to carry each ancestor's application; Core re-asks as a chained request when a stack wants details its request did not fetch.
- Questions: which settings a profile carries; the say-all trigger (`speech/sayAll.py` 243 to 246); which stack a focus request's settings come from before the answer names the new focus's ancestors. Not precluded: a switch changing the UI Automation selection settings clears the backend cache (`config/configSpec.py` 301 to 310); an exited process's trigger kept until the next commit (`api.py` 102 to 103); the view's keyboard settings written by the switch stage; profiles prepared off the reducer thread. Triggers are suspended while an NVDA dialog is shown, not focused (`gui/__init__.py` 1008 to 1021).

M8.4, the highlight: draws only from rectangles answers carry (focus answer: the focus node's; navigation and node reads: the node's; ancestors and bulk reads none, `verbatim-outpost/src/gates.rs` 18 to 24); follows nothing and re-reads nothing on a move or scroll, as NVDA registers four events (`vision/NVDAHighlighter.py` 512 to 515). Not precluded: a caret answer carrying a rectangle, if review follows the caret (`api.py` 284 to 290). The screen curtain's checks run on a shell thread. DPI between outpost and overlay not verified.

M8.7, UIAccess and secure screens:

- Builds on what build 1 adds: the input-desktop fact, the at-once switch, the sleeping focus, the view's desktop and lock parts, the lock state. "Secure Desktop" (security mitigation B11; NVDA `secureDesktop.py` 53) is M8.7's; until then a switch is silent apart from the cut. Also M8.7's (B4 to B8): the allowlist's content, navigation stopped at the lock screen application's boundary (`NVDAObjects/lockscreen.py` 17 to 33), the lock app module's navigator handling, braille cleared at the lock.
- To verify first: Microsoft's "Security Considerations for Assistive Technologies" (cited, not fetched) says a UIAccess program started by a user outside the administrators group runs at medium-plus integrity and cannot reach high-integrity windows. If so, the hook's integrity guard and the view's integrity part stay for standard users, M8.7's exit (`docs/roadmap.md` 1619 to 1620) holds only for an administrator's session, and that scope is Dickson's. Today's comparison already holds keys only when the window's level is known and no higher than Verbatim's (`verbatim-core/src/state.rs` 693 to 704).
- The listener, every outpost and Core's hook need UIAccess together, since the listener holds every application's events; a listener without it silences every elevated window's events at once.
- An elevated process's exit may never be told if the supervisor cannot open it, as the outpost's target watch cannot (`verbatim-outpost/src/outpost/window.rs` 193 to 207; not verified for the supervisor).
- A request to an elevated window fails quickly and is filed as MSAA for half a second (`verbatim-uia/src/probe.rs` 38 to 40, 50 to 54); an answer "unreadable: higher integrity" would let Core stop asking.
- Verbatim's own injected keys (Windows+U for Ease of Access, `easeOfAccess.py` 66 to 95) pass as its own input with no entry. The secure instance has its own Core, hook, listener and outposts and no control plane (C1). Braille gives the display up at a switch only when registered with Ease of Access (`braille/brailleHandler.py` 205 to 218), with M18.

M12, Remote: a remote key enters through the keyboard hook, injected as NVDA does on its main thread (`_remoteClient/localMachine.py` 348; `_remoteClient/input.py` 130 to 149), then held and echoed like a local key. Not precluded: a view part for a capture mode that swallows every key; a captured key that neither cuts speech nor joins the queue, forwarded from the inbox (NVDA's decider runs before the cut, `keyboardHandler.py` 214); a speech tap after the manager's fence (NVDA taps `pre_speechQueued`, `_remoteClient/session.py` 346); policy facts the injection point can read. Questions: where the leader's key is forwarded from; whether Remote shares the control plane's vocabulary; whether an injected key's origin can be proven (trust).

M13 and Tier D, applications' object models: not precluded: an object-model handle held behind an id with its generation; the text provider named per request (Scintilla's window messages, `NVDAObjects/IAccessible/scintilla.py` 140, 243; Notepad++, LibreOffice, Kindle); a named program run whole in the outpost; reads that must run in process answered by the outpost through its helper, never the helper answering Core; Word's paste group re-reading the caret (`winword.py` 492 to 508); a watch on any named value with a bound per module (Office's movement and format keys); an application's own event source held by its outpost and re-registered when the worker is replaced (PowerPoint's sink). Word on MSAA (`winword.py` 33 to 490), Excel's borders (`excelCellBorder.py` 112 to 168), charts (`_msOfficeChart.py` 517 to 583; NVDA's axis argument swap at 975 to 993 is a capture question), Visual Studio's DTE (`devenv.py` 74 to 214). Documents (M6.5, M7): table navigation as one request run whole with Core's true row and column (`documentBase.py` 347 to 489, not verified); say-all by row or column; layout tables from an answer flag; indentation as Core's per-object memory (`speech/speech.py` 1882 to 1904). Where a module's caret-key declaration with host text runs is M13's.

M14, maths: the engine is a request target that is not a reader, asked with a due time and Cancel, its replies recorded as inputs (an engine called inside the reducer breaks determinism, `architecture.md` 622 to 625); it keeps one expression, its position, placemarkers and modes for the whole process (`mathPres/MathCAT/MathCAT.py` 120, 147, 195, 213 to 214, 273, 413, 466, 494) and parses untrusted markup (63 to 75). Entering commits a focus Core commits itself whose kind selects 69 gestures; Escape commits the earlier focus again with one read (`mathPres/__init__.py` 156 to 171, 185 to 187); the readers' last focus stays the document, so its re-raised focus is a duplicate. MathML is a node property (`NVDAObjects/IAccessible/ia2Web.py` 387 onward); maths inside text needs a chained engine request (`speech/speech.py` 925 to 934, 1524 to 1535). Questions for Dickson: where the engine runs; the highlight's per-node rectangle map, a bulk location read against "Bulk reads carry only cheap properties" (`architecture.md` 737 to 749; `_navNodeMapping.py` 116 to 156); speech items `SpeechItem` lacks (breaks, volume, rate, character mode, phonemes, language change, tones; `verbatim-speech/src/driver.rs` 34 to 51), a synthesizer contract change. Copy as MathML, LaTeX, ASCIIMath or speech is a shell effect plus engine requests (`MathCAT.py` 245 to 375).

M18, braille: the driver's channel is the extension API's contract, not a reader protocol. Not precluded: display gestures as inbox inputs that mark caret keys and cut at their turn; emulated caret keys sent and watched by Core; a flag on caret, focus and review reads asking for braille's unit with fields and the absolute selection, minting an anchor held while shown; an "activate at this position" request; gestures carrying the generation of the output they address; the MSAA change rule naming braille's shown node besides the focus ancestry; a foreground progress bar's dialog refresh at most once a second (`braille/brailleHandler.py` 903 to 920); braille's timers, the blink preferably in the host; typed-character entries for characters Core injects (whether injected Unicode reaches the hook's echo, not verified). Question: what the display shows during a terminal flood (Core reads only when the speech flood policy allows; NVDA refreshes per change, `behaviors.py` 532 to 537, `winConsoleUIA.py` 494 to 504).

Report focus and the control protocol: whether report focus speaks the application's live focus (today) or the committed focus (NVDA, `globalCommands.py` 2957 to 2979) is its milestone's. The control protocol's version 5, a public contract, is proposed and not decided: `FocusReport` names the committed focus plus pending entries, the asked request with its due time and held ids; a latency stage "event queue" and a "no longer awaited" end; `live_re_reads` removed; the event stream carries executed entries, with dropped, merged, given-up and refused tells on a separate subscription, so a background flood does not fill its 256-frame queue and disconnect a subscriber (`verbatim-control/src/server.rs` 87, 1204 to 1217); `DumpTree` outside the event queue.

M6 items: M6.1 measures the listener's per-event cost. M6.5's formatting raises caret events of its own, each told and read once. M6.6's background flood cap is a background class of the event queue. M8.1's input help and layers need the hook's view to carry them, and its "modifiers left held after the secure desktop" item (`docs/roadmap.md` 1478 to 1482) takes its signal from the view's desktop part. M8.2 and M8.5 (speech history, recognition results) are objects with no reader behind them; a recognition result is committed as a focus Core commits itself.

## Statements found contradicted

Statements in the project's documents that the pinned NVDA source or the code contradicts, each for the session that next changes the document.

None of these was changed; each is for a later session to correct.

- `docs/nvda/braille.md` 25 to 27: "each cell remembers the text position it came from (`regions/_routing.py`)". `_routing.py` holds only the caret-routing setting test (13 to 25); the mapping is the region's raw-to-cell maps (`braille/regions/base.py` 63 to 66, 122 to 138) resolved through `regions/textInfo.py` 358 to 392.
- `docs/nvda/braille.md` 34 to 35: a message is dismissed by "the next content write" only if a key was pressed since it was shown (`brailleHandler.py` 829 to 830, 884 to 888).
- `docs/nvda/braille.md` 45 to 46: the handler subscribes to nothing; the object event handlers call it (`NVDAObjects/__init__.py` 1355 to 1429).
- `docs/nvda/braille.md` 56: `source/brailleInput.py` is a deprecation shim; the handler is `braille/input/inputHandler.py`.
- `docs/nvda/process-injection.md` 52 to 53: the helper registers one in-context hook for every event of the process (`injection.cpp` 159), features registering callbacks in a table (`inProcess.cpp` 78 to 93).
- `docs/nvda/process-injection.md` 108 to 109: "injection does not cross ... into AppContainer": the helper loads there and only then skips the IAccessible2 proxies (`IA2Support.cpp` 130 to 145, 164 to 170).
- `docs/nvda/process-injection.md` 54 to 56: the proxies are registered unconditionally on each UI thread at its first focus or foreground event, except in suspendable and AppContainer processes (`IA2Support.cpp` 98 to 107, 147 to 173).
- `docs/nvda/math.md` 4 and `docs/parity/text.md` 666 to 667: maths called unscheduled, though M14 schedules it (`docs/roadmap.md` 1778). `math.md`'s "navigation state lives in the provider" omits the source object, parent and rectangle map NVDA keeps.
- `docs/roadmap.md`, "MathML and LaTeX reading": the pinned NVDA reads no LaTeX input.
- `docs/roadmap.md`, M7's echo item: see M7 above (`ime.cpp` 496 to 501; `tsf.cpp` 490 to 499).
- `docs/design/ownership-milestone-rules.md` 55: profile triggers are suspended while an NVDA dialog is shown, not while it is focused (`gui/__init__.py` 1008 to 1021).
- `verbatim-outpost/src/text/mod.rs` 19 to 21 and `verbatim-model/src/text.rs` 59 to 62: an unheld anchor "is forgotten once 64 newer ones were minted"; no such limit exists, it is freed at the next release once neither Core nor an unacknowledged message names it (`verbatim-outpost/src/store/mod.rs` 505 to 521).
- `books/dev/src/explanation/architecture.md` section 10 and D8: two endpoints and "JSON-RPC over named pipe"; `verbatim-control` has one pipe (`protocol.rs` 39 to 40), a custom newline-delimited JSON protocol, no install request (`protocol.rs` 50 to 122) and no developer-mode gate.
- `docs/nvda/virtual-buffers.md`: the change notice "re-syncs open TextInfos", `_handleUpdate` "re-anchors state", an RPC that can "activate a node", and a line as "one cross-process call" are wrong against the pin.
- `docs/nvda/browse-mode.md`: NVDA+Space's script is misdescribed, and `autoFocusFocusableElements` does not exist at the pin.
- `docs/nvda/ia2.md` 59 to 62: the in-process hook's events are misdescribed.
- `docs/nvda/speech.md` 371 to 372: say-all does not detect a changed document as stated.
- `books/dev/src/explanation/design/key-holding.md` 20 and 42, `docs/roadmap.md` 1596 to 1603 and `verbatim-win32/src/integrity.rs` 10 to 11 say the integrity guard is removed when UIAccess is built; Microsoft's page (cited, not fetched) says a standard user's UIAccess cannot reach elevated windows. `books/dev/src/explanation/windows/processes-and-security.md` 18 to 30 has been cited both ways; verify before correcting any of them.
- `docs/parity/text.md` 85 to 99, 134 to 136, 186 to 189, 293 to 309, 498 to 505 and the progress-bar design's line 87 become false as builds 1 to 3 land; correct each with its build.
- `docs/parity/focus-context.md` says NVDA speaks the focus alone on the secure desktop's return; NVDA's sleeping object has no parent, so NVDA says the window's title and the focus, as Verbatim does.
- Parity ledger gaps to record: NVDA says an attribute's negative when it disappears ("no bold", `speech/speech.py` 2909 to 2920) where Verbatim is silent (`verbatim-core/src/text.rs` 350 to 354); the Caps Lock lowercase beep is NVDA's behaviour and missing.

## Evidence

- Facts with more than one owner, and their defects: `architecture-study.md`, section 1.
- The back-channel's chain: 43de3932, 8fe59b1b, d893e07f, c0bbf19b; and `unbuilt-focus-designs/core-flow-review.md`, findings 1, 3 and 5.
- The foreground: b1917eed; audit difference 12; 06594d66 and 9661f0d8; 5f1c8e81, 3578a437, e60a7a58, 38cab859.
- The gap's moves: 01cba97e, ad78f2a8, 9ba7625b, 5242658c, 28be3d08, 22256896, f2f8209a, e02c7854.
- The publication's rounds: c47d57ae, 6dbcca25, 2bd69a85, 8a087fe7.
- The listener's scheduler: 5204f351, e46d4183, 0b5826cd, 073d63b0, 69c06239.
- Verdicts: 094a8dc8, 07db29a8, a3d6bc23, 248f85d3, defect B.
- Identity: b94d2ea2, 9fd28a66, 8826d457, 5797c075, and defect A from 3abda293 with 10c9e83a.
- Lifecycle: 1a3dc022, 8d01bbd1, a39f01f8, de7f481e, df4c15d9, ded1988a.
- Requests: 886d8e73, 76180512, 348262b8.
- Merges: dd363ecc, 913bb0e8, 89bbbc7c, 71322fae, 06256225.
- Lent state: d0ec3a89, the anchors' follow-up, 2bd69a85's terminal copy.
- Input order: bf4fbedb, ba7064e5, e180f14a, 04214e7f, 49fcca0e.
- Output: 3ebb7be0, 21fb0e51, 6e303a61, 0d791a13, 2bdfffb7.
