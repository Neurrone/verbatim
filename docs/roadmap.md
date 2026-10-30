# Verbatim Roadmap

Milestones are scoped by _risk retired_, not calendar time. Each has explicit
exit criteria so progress is testable. Ongoing tracks (localization,
observability, latency budgets) start early and run through every milestone
rather than being milestones themselves.

## Status

M0 (foundations), M1 (self-voicing prototype), M2 (test harness and
VM), and M3 (desktop usability core) are complete, and what each
delivered is in [Completed milestones](roadmap-done.md). M4 is in
progress; its completed parts, M4.1 (stability) among them, are listed
there too, and M4.1's open follow-ups are under M4, "Follow-ups from
M4.1".

## M4 — Text, editing, and terminals

- TextPattern support in the model; caret tracking, typed-character echo,
  word/line/character navigation; say-all with index-mark continuation.
- Text runs carry formatting attributes as utterance spans (spelling and
  grammar markers, and font/color where exposed), so M11's
  formatting-change sounds have data to act on.
- A minimal built-in character-description table (punctuation and symbol
  names): character navigation must say "comma" on a comma even though the
  configurable dictionary system waits until M8.
- Remote-ops integration (ancestor fetch on focus; terminal text ranges).
  The ARM64 remote-ops verification (R3) is not part of M4: it was
  deferred on 2026-10-06, and M4 is built and verified on x64 only.
- Windows Terminal: diff-based output announcement with flood policy; the
  "cat a huge file" scenario is an E2E latency test.

Exit: Notepad editing is solid, and the focus pipeline and caret and
selection reworks are built.

M4 ends when the two groups below land: the focus pipeline rework, then
the caret and selection rework, each in a session of its own (Dickson,
2026-10-25). Within a group the items are in the order they are done
(Dickson, 2026-10-10 and 2026-10-13). The rest of the open work found
during M4 is scheduled in the milestones that follow, and a Deferred list
closes the section with items that are not M4's. Each item is one
change, checked with `cargo xtask ci` and its related scenarios ten
times, with the full suite where every scenario is affected. A change in
what is heard goes to Dickson first; while he is away, it follows NVDA
where NVDA is clear, recorded as provisional.

### The focus pipeline rework

Designed, reviewed and agreed with Dickson on 2026-10-29: every
reconciliation between Verbatim's processes that the study of 2026-10-25
found, so that each mechanism left either goes or is the only one of its
kind, and every non-deliberate difference from NVDA's focus handling in
[the focus pipeline audit](design/focus-pipeline-audit.md). The shape
stays (the listener reads every UIA focus, the outposts the rest, Core
decides). The decisions, the design and the order of work are in
[the focus pipeline rework](design/focus-pipeline-rework.md), the one
source for them; its first step is the measurements and captures.

### The caret and selection rework

The decisions, the design and the order of work are in
[the caret and selection rework](design/caret-selection-rework.md), consolidated for building, the one
source for them; its first step is NVDA's captures of the keys not yet
captured. Agreed with Dickson on 2026-10-25 and 2026-10-29 after a study found the design's shape
right (a held key's caret read before it, the outpost watching for
evidence and comparing positions in the provider, Core choosing every
word) but a second layer, added before key holding closed the race it
answered, never removed: the caret and selection were kept in nine
places, and two known defects come from copies disagreeing (Control+A
followed by a repeated focus says nothing; a superseded key's answer
leaves Core's caret stale).

The decisions, in brief:

- Pin today's defects with tests first.
- One last read per text node in the outpost as the only baseline, one
  request per key that reads the caret before it and opens the watch, and
  Core taking every report and answer as the newest caret by one stamp,
  the read's start; the baseline's life follows the focus Core holds.
- Remove the second layer: Core's eight caret reports and its Backspace
  and Delete prediction, the outpost's eight reads (so that until
  uiAccess, M8.1, a caret key in an elevated window is judged from the
  last read), the time-zero record, the line-limited "since" check,
  report numbers, the selection baseline's three modes, the separate
  watch request, and the single superseded slot.
- Control+Z, Control+Y, Control+X, Control+V and Escape in a text field
  match NVDA: not held and silent, replacing the decision of 2026-10-08
  that spoke them; the `caret_keys` scenarios' expectations change.
- A key that moves nothing speaks as NVDA's script does, so that Up
  Arrow on a one-line field reads the line, replacing the decision of
  2026-10-08 that made it silent: at once at the edge of the text when
  nothing is selected, otherwise 100 ms after the key; a move that
  arrives later, within the watch's new 1-second bound, cuts speech and
  is spoken. Alt+Up and Alt+Down Arrow move by sentence where the
  application does not, as NVDA does. Terminals keep their reading
  rules for all of this (Dickson, 2026-10-29).
- One time stamp per caret report; the outbound queue's clause against
  merging a caret report across a reply is removed, its stated reason
  predating 505e3ad8.
- The judgement of a key's evidence stays in the outpost, named in D14 as
  the one read-time decision outside Core, since it bounds reads on text
  changes.

### Deferred

- Review keys straight to the reducer
  ([communication](design/communication.md), section 11) is reconsidered
  when the command set is settled (Dickson, 2026-10-14).
- A question for Dickson: `notepad_editing` reads Notepad's spelling
  marks once before its second Control+Home, since Notepad raises no
  event when it marks a word, and fails when the idle-time spell check
  has not marked "gammaelta" yet; is there a sanctioned form of wait for
  a precondition with no event?

### Follow-ups from M4.1

M4.1's follow-ups still open or deferred, each with its next action.
Unless an entry names a scenario or an `experiments/` folder, no evidence
is kept in the repository.

Tests and tooling:

- Two mockapp tests fail intermittently under `cargo xtask ci`, a defect by `docs/testing.md`: `call_counts::uia_focus_chains_stop_below_the_process_boundary` and `shutdown::a_remote_operation_in_progress_finishes_before_the_outpost_shuts_down`. Find each cause. The first's counts hold with a 1.5 s pause between each of its steps, so UIA's lapsing element state is not its cause.
- Audit every end-to-end scenario for timing races, each fixed by waiting for evidence, not a longer bound; find why the agent's bring-forward always takes its slowest path (up to 1.4 s). `measure_focus_explorer`'s key waits end on their key's item, which two keys share ("alpha 2 of 4" for Down 1 and Up 1), so a late repeat can end the next key's wait; the targets are `explorer_folder_window`'s, so renaming cannot fix it.
- Root-cause, from the GitHub run logs, the ARM64 unit test `the_codeview_record_read_is_the_one_dbghelp_reports` (`verbatim-trace-waits`), which failed on main before 2026-10-15 and does not reproduce here.
- The trace tool's symbol lookup for ARM64X images, such as ntdll, needs a fix testable only on the ARM64 runner (deferred, Dickson, 2026-10-21).
- Compare mode presses Verbatim+V (`kb:verbatim+v`) as Insert+V, which under NVDA types "v" into Notepad and corrupts the later steps of `notepad_editing`'s comparison.
- The platform-neutral dependency check (`check_platform_neutral_deps` in `xtask/src/main.rs`) runs `cargo tree -e normal`, so a build-dependency on `windows-sys` or a Windows-specific workspace crate is not caught, `winapi` is on neither banned list, and a dependency on a Windows-only workspace crate that uses no bindings itself passes.
- `cargo xtask trace scenario` and `cargo xtask measure` look for the build under `<worktree>\target` and ignore `CARGO_TARGET_DIR`, so a worktree building elsewhere measures a stale or missing build.
- A double-click is checked only by hand: the agent has no mouse request, so no scenario pins "beta selected" for a double-click inside a word.
- Key holding has no test of a waiting key following the newest watch (`crates/verbatim-core/src/editing.rs`); its other gaps are in M6.5.

Crashes and stalls:

- Verbatim's own outpost once left a fact in its intake for 6.37 seconds; reproduce it with the intake and menu logs.

Focus and File Explorer:

- Core drops a changed description that equals the name
  (`reduce_description_changed`), where NVDA speaks a changed description
  even then (`docs/nvda/speech.md`); found 2026-10-30.
- A scenario's final check that nothing more was said sees only events an
  application raised before Verbatim answered the idle barrier, so a
  focus raised just after, such as File Explorer's focus on the Start
  button after Escape closes the Windows+X menu, let `quick_link_menu`
  pass in some runs while expecting silence where NVDA and Verbatim both
  speak (found 2026-10-30); a scenario expecting silence after a key needs
  evidence that the key's effects have all been raised.
- `cargo xtask nvda compare quick_link_menu` flattens NVDA's Windows+X
  item to "Installed apps Alt, p1 of 17" where the scenario has "p 1 of
  17", so the step is judged "differs"; find whether the compare tool
  joins NVDA's speech parts without a space or NVDA runs them together.
- File Explorer can focus its "Working on it..." placeholder and raise no later focus, UIA or `WinEvent`, so `measure_focus_explorer` times out, which decision 11's wait cannot recover; find what File Explorer raises then, beyond a name change and three state changes on the items view. On 2026-10-29 it failed this way in 22 of 23 runs in a row on the development machine, on builds before and after that day's changes, while `explorer_folder_window` passed, after a "Low disk space" dialog had taken the foreground during a batch; the state that makes it constant is not found.
- In one runner run the `delta` item inside Inner still carried the parent folder's window title; check whether File Explorer's title lags or an ancestor name is stale (`explorer_folder_window`).
- The listener's read in File Explorer scenarios once came 285 ms late, locally mostly the first nearest-window lookup waiting on a busy File Explorer; confirm on GitHub's runner (deferred, Dickson, 2026-10-21).
- On a fresh GitHub runner the first Verbatim start can say "Start toggle button not pressed" instead of "Desktop list", because the taskbar rehosts the Start button in a new PopupHost window and raises a UIA focus NVDA would accept too; the choice between a throwaway warm-up start and an experiment to find the trigger is deferred (Dickson, 2026-10-21).
- A focus-now answer carries no application name and no Alt state, so an application rule such as the Alt+Tab switcher's cannot apply to a focus read on request.

Events and controls:

- The classic .NET Framework Windows Forms combo box rule, element-selected events from the `ComboLBox` window reported as the combo box's value change (`NVDAObjects/UIA/__init__.py`, lines 2734 to 2744 and 2779 to 2790), is deferred until Verbatim reads such a combo box in an application (Dickson, 2026-10-20).
- The rule for a `Windows.UI.Core` combo box with the `Value` pattern (lines 2795 to 2798) is not built, since every Settings combo box checked lacks the pattern (`docs/parity/changes-and-selection.md`, "A combo box's value from its selected item").
- The events NVDA registers desktop-wide that Verbatim lacks are deferred until each is supported (Dickson, 2026-10-20; `docs/parity/backends.md`, "UIA event registration"); a foreground UIA progress bar that is not the focus is in M4.2, background progress bars in M4.3.
- Table cells: the suppression of an unchanged row and column within the same table is not ported (pending Dickson), nor are header texts, spans, a cell's own coordinates text, or MSAA and IAccessible2 cells.
- On this Windows 11 build `TrayNotifyWnd`'s UIA subtree is empty, so the tray list (`crates/verbatim-outpost/src/outpost/shell_items.rs`) is empty unless the overflow flyout is open; and a double press of Verbatim+F11 opens the tray list rather than the taskbar list.

Text and selection:

- `text_box_selection_keys` fails at Control+Delete over the selection "alpha " in some runs, saying "alpha" where "beta", "selection removed" is expected: the key's watch answers from its first check, before the edit control has handled the key. The caret and selection rework removes the first check and runs the scenario ten times; if it still fails, the likely cause, unconfirmed, is Core comparing "alpha" while the outpost's word unit reads "alpha " with its white space.
- The parity ledger's statement that Alt+Up and Alt+Down Arrow speak the line when the application moves the caret has no capture behind it; the caret and selection rework's captures settle it.

## M4.2 — Parity small items

Small differences from NVDA found during M4, each closed against a live
NVDA capture.

- A foreground UIA progress bar that is not the focus is reported, as
  NVDA does (Dickson, 2026-10-10, as a separate fix); background progress
  bars are not yet reported (M4.3).
- Toasts ([parity](parity/alerts.md), "Toasts"): Verbatim reads them only
  through an MSAA alert, NVDA through UIA's window-opened event. Capture a
  Windows 11 toast live with NVDA and with Verbatim; if Verbatim is silent,
  follow NVDA (a `Windows.UI.Core.CoreWindow` whose automation id contains
  `ToastView`, from a desktop-wide window-opened subscription in the
  listener), then adopt NVDA's rule dropping MSAA alert and show events
  from native UIA windows.
- Selection inside a list the focus controls: the
  `settings_search_suggestions` scenario covers it, compared with NVDA
  on 2026-10-20. Left to check against a live NVDA capture are the
  differences the parity entry records from the source alone ([changes
  and selection](parity/changes-and-selection.md), "Selection in a list
  the focus controls"): a stale selection after a second replacement of
  the list, a selection in a second controlled element, and a selection
  raised on the controlled list itself.
- The review cursor on objects that no longer exist (Dickson,
  2026-10-08): most review commands answer from Core's cached line or the
  node's snapshot, and a Gone answer is dropped with the navigator left on
  the dead node, so a closed tab's or dialog's old text can be read where
  NVDA, reading live on every command, gives silence or "blank". Review
  commands read live from the application rather than from Core's cache;
  a Gone answer for the navigator's node re-seeds it from the focus, or
  says "No navigator object" when the dead node is the focus; the outpost
  reports held MSAA nodes it forgets with their window; and an end-to-end
  test in both terminals that a reviewed line stays on its text as new
  output is written below it without filling the history. The review
  position is the provider's, with no bookkeeping of Verbatim's own: once
  the history is full, staying on the same row is acceptable.
  Flat review's current-line command (`crates/verbatim-core/src/reduce.rs`,
  the flat `ReviewCurrentLine` arm) moves the review offset to the line's
  start, where NVDA's current-line command does not move the review
  position; it is fixed with this item ([review parity](parity/review.md)).
  Object navigation from a node whose element is gone: Verbatim moves the
  navigator to the focus and announces it, where NVDA says "No next" or
  "No objects inside", or takes the window for a parent step
  (`NVDAObjects/UIA/__init__.py` 2136 to 2196); settled here against a
  capture. Such answers are more frequent since the outpost's runtime-id
  search was removed (719b4ed4, 2026-10-29), so a node with no kept
  element and no focus route answers gone.

The remaining work is two sessions (Dickson, 2026-10-21): one takes
toasts, the foreground UIA progress bar, and the differences in a list
the focus controls; the other takes the review cursor on objects that no
longer exist.

Exit: each entry is matched (verified) by a scenario, with a declared
deviation where Dickson chose one, checked with compare mode.

## M4.3 — Settings and indications

These are decided designs that daily use needs.

- The settings package (Dickson, 2026-10-08, from an inventory of NVDA's
  242 user-visible settings against Verbatim's): the
  missing settings for behaviour Verbatim already has (capital pitch
  change, OneCore's pause after punctuation, the audio output device,
  sounds' volume following the voice,
  handling keys from other applications, the multiple key press timeout,
  the review cursor following focus, reporting tooltips, reporting
  notifications, the word segmentation standard, cancelling speech for
  expired focus events, trimming leading silence); dialog controls for
  the nine settings that exist only in `settings.toml`; and three small
  features (say "cap" before capitals, delayed character descriptions,
  the spelling-error sound while typing). The review cursor follows focus
  and the caret by default, each with NVDA's setting to turn it off.
  Spoken progress comes every 10 percent, as NVDA's does. Each of NVDA's
  settings for a feature not yet built lands with that feature's
  milestone.
- Status cues (Dickson, 2026-10-08): every indication in
  the theme's Events group (start and exit, error, application not
  responding, browse and focus mode, suggestions opened and closed,
  progress) plays through key presses; indications in every other group
  are content and are cut off. This is built-in behaviour defined by the
  group, not a setting.
- Sound-only announcements (Dickson, 2026-10-08): sounds without speech
  are normal, since a theme can set any indication to sound alone, so an
  announcement a theme reports by sound alone is queued, timed, measured
  and cancelled exactly like a spoken one; the start-up sound becomes
  one.
- Background progress bar reporting with its setting, an NVDA option off
  by default (Dickson, 2026-10-10, confirmed as planned for later on
  2026-10-15; [changes and selection parity](parity/changes-and-selection.md),
  "Value change on focused node speaks bare value").

Exit: the settings dialog covers the settings package's inventory, and
every indication plays or speaks as its cue says.

## M4.4 — Message types in their final home

Before M5, because M5's extension API exposes events and commands, and the
types they carry should have their final home first.

- The messages exchanged with the outposts and the listener move into a
  platform-neutral crate as plain serde types, now that the communication
  migration is built (the communication design's decision 13, approved).

Exit: the messages exchanged with the outposts and the listener are the
platform-neutral crate's types.

## M5 — Extensions v1

The design is [extensions](design/extensions.md); the order of work, with the
tests that prove each step, is [the M5 work plan](design/m5-extensions-work.md).
This entry keeps the scope and the exit criteria. M4.4 is the plan's
precondition, because the extension surface exposes the messages it moves.

- Modules as folders: a module is a `module.toml` beside a Wasm component;
  a built-in is a module folder under `extensions/builtin/` (planned), never listed
  and never disabled by the user; an extension is a `.vext` bundle of
  modules, installed by a request to the running Verbatim through a small
  helper executable and the control plane (NVDA's `nvda_slave.exe`
  pattern), never by a flag on `verbatim.exe`, whose single-instance
  takeover would kill the running instance.
- Rules as data: a matcher and an action with a required id and an explicit
  backend, scoped to the sender's application identity, applied by the
  outpost before any hook runs, refused at load when two rules of one module
  contradict.
- The component host on wasmtime with the component model (D6, decided
  2026-10-22): epoch deadlines re-armed per hook from the outpost's
  watchdog, node reading as bindings over the outpost's own readers, speech
  enforced by attention, hooks `on-load`, `on-unload`, `on-event` with
  `pass`, `drop`, `replace`, `rewrite` and `adopt`, `on-node`, `status-bar`
  and `on-command`. The `ci-arm64` job ratifies Cranelift on ARM64 and
  measures host-call overhead against the hook deadlines; Pulley is
  switched on only if that ratification or a Cranelift fault requires it.
  A hook's time is a stage in the latency timeline and two rows in the
  performance ledger.
- Capabilities derived from the component's imports, never declared; the
  install and Extensions dialogs show only the sentences naming the user's
  things or code outside the sandbox.
- Binding by application identity (executable, console client, hosted app,
  WebView2 parent) resolved by the supervisor, one module instance per
  outpost; hot reload of installed, developer and built-in module folders
  by validate-then-swap.
- The minimal log ring, the log viewer window and the Verbatim+F1 focus
  report, pulled forward from M9 because module failures and rule
  provenance need somewhere to be read.
- The guest workspace under `extensions/` (planned) with a permissively licensed SDK;
  every built-in written in Rust, Calculator and Notepad first.
- Every Windows inbox application as a built-in: Explorer (starting with the
  cosmetic fixes deferred from M3), Windows Terminal, Notepad, Calculator,
  Task Manager (proven by `mockapp` until M8.1 lets it be read end to
  end), Settings, Start and search, Quick Settings and Open With; sleep mode
  as a core per-application list rather than five modules.
- Windows Terminal's second terminal identity
  ([design](design/windows-terminal-twin-focus.md)) as the Terminal
  built-in's `on-event` hook, never core code: it drops a sender with the
  runtime id starting 14 that is the twin of the current focus starting
  42, and adopts a 42 sender that is the twin of a current 14 focus, by
  the design's twin test, with tests that the terminal is followed
  afterwards and that cover two panes in one window. Before implementing,
  collect evidence that the runtime id's last part is never reused (pane
  splits, pane moves, closing and reopening panes, new tabs). Until then
  the tests accept one repeated "terminal" as an optional utterance, which
  does not cover the late repeat of a 42 to 14 to 42 flip seen on GitHub's
  slower runner, so that can still fail scenarios. Evidence:
  `experiments/windows-terminal-raises-focus-from-a-second-runtime-identity/`.

- Focus vetoes and redirects with no read NVDA would not make (Dickson,
  2026-10-29; [the focus pipeline rework](design/focus-pipeline-rework.md),
  "Room for extensions"): a veto declared as a rule judged by the listener
  from the event's cache; a hook's code veto or redirect run in the
  outpost after the listener's read, as NVDA runs `event_gainFocus` after
  its walk, so it makes no read NVDA would not; node and event rules
  applied in the outpost to every kind of focus report, its own recovery
  reads included; kept verdicts dropped when rule tables are reloaded; the
  Windows Terminal second-identity hook restated against repeats judged in
  Core; and File Explorer's search band refusal (the focus pipeline
  audit's difference 24) as a rule of the File Explorer built-in, needed
  on Windows 11 24H2 and not on 25H2, with no `StartButton` rule, since
  the rework's first step found no keyboard route to the MSAA focus on
  File Explorer's `Start` window.

Out of M5 by decision (Dickson, 2026-10-22): module settings and storage,
the utterance, settings-changed, timer and response hooks, asynchronous
imports, global modules, updating or reordering installed extensions,
scripted modules and the interpreter console (M10), and the
`verbatim:synth` world with its tone synthesizer (M6, where the host work
is).

Exit: an app module can be edited and hot-reloaded without restarting
Verbatim; a host function a module did not import is unreachable from it,
a surface item outside the design is refused at load, and both are tested;
the inbox built-ins' scenarios are green. This milestone opens the
app-module porting track (see the section after M15), which then runs
continuously alongside every later milestone.

## M6 — Native synth host + Eloquence

This milestone comes right after M5 and before browse mode so that
Eloquence can be delivered through the extension mechanism and
Verbatim can become Dickson's daily screen reader sooner (Dickson,
2026-10-15). The design is section 13 of [extensions](design/extensions.md)
and D18; the order of work, with its tests, is
[the M6 work plan](design/m6-native-synth-work.md).

- The `verbatim_synth_v1` C ABI, the `SynthDriver` trait spelled as C
  exports, and the adapter crate that generates them; the `verbatim:synth`
  world with a tone synthesizer component, hosted by the same
  `verbatim-synth-host`, so Wasm and native engines share one host from
  the start.
- The host loads adapters matched by architecture; a reduced x86 host
  carrying only the adapter loader and the protocol, cross-built for
  `i686-pc-windows-msvc` by `cargo xtask`, serves 32-bit engines. An x64
  adapter on ARM64 is unsupported until an x64 cross-build is added.
- The AppContainer in its less privileged form wherever the engine allows,
  each hosted engine declaring the strictest level it works under, the
  container's access entries set by the contained launch itself, and a
  probe child asserting every denial.
- The Eloquence adapter, written from IBM's documented SDK as
  `eloquence-x86.dll`, with the engine files supplied by the user; the
  `eloquence_` scenarios run by Dickson by hand with the engine files,
  never in CI.
- A shared-memory PCM ring only if the pipe path misses the budget for
  Eloquence; pipes until numbers prove otherwise (D18).

Exit: Eloquence speaks through Verbatim under the sandbox; the latency
scenario is green in Dickson's run of the `eloquence_` scenarios, recorded
here.

## M6.1 — MSAA, UIA and Windows call cancellation

Scheduled after M6 and before M7, whose IAccessible2 work adds many COM
calls into browsers (Dickson, 2026-10-29). Separate from the focus
pipeline rework, since cancelling a stuck call frees a thread that is now
abandoned but changes nothing spoken.

- An experiment on whether `CoCancelCall` ends a UIA client call, as
  `experiments/cocancelcall-ends-an-msaa-com-call-but-not-a-window-message/`
  did for MSAA; the decision of 2026-10-21 not to build MSAA cancellation
  stands until then.
- If built: cancelling the outpost worker's and the listener's reading
  threads' stuck calls, as NVDA's watchdog does, and what that lets go:
  abandoned threads, the bound on an application's abandoned reads, and
  the 11-second `READ_DEADLINE`.

## M6.2 — Outpost retirement

At most one outpost serves an application at any time (Dickson,
2026-10-29): a failed or retired outpost is ended, killed if it does not
exit, before its replacement starts, because a failed outpost is retired
on a thread of its own while its replacement is started after the backoff
(`crates/verbatim-outpost/src/supervisor/owner.rs`, the failure path),
so for up to `SHUTDOWN_LIMIT` a stuck outpost and its replacement can
both be in the application, where two Verbatim processes' calls collide
inside UIA (`docs/design/listener-without-connections.md`).

- A mockapp test that fails an outpost during a held remote operation and
  checks what its replacement reads.
- The replacement started only once the old process has exited, and the
  kill's place in that order, weighed against the crash inside Windows
  Terminal's `UIAutomationCore.dll` that the clean shutdown was added for.
- `SHUTDOWN_LIMIT` (21 seconds) rests on UIA's default transaction timeout
  of 20 seconds, which Verbatim lowered to 5 seconds on 2026-10-10, and a
  timeout bounds each provider call, not an operation; its bound is
  settled with the order above.

## M6.3 — Speech and audio pipeline

The speech and audio pipeline looked at as a whole, after M6 adds a
second real engine (Dickson, 2026-10-29).

- An audit of the speech and audio pipeline against NVDA, as the focus
  pipeline's was made: for each step, the rule, the order, what happens
  on a slow or failed engine, and what is heard, across the speech
  manager's priorities and cancellation, the culling of expired focus
  speech, index marks and say-all's continuation, the synthesizer host
  and the mixer; every difference no decision covers is fixed here.
- When anything handed to the synthesizer or the mixer has expired,
  everything handed on is stopped and the still-valid utterances are
  handed on again, as NVDA's speech manager does; today they are lost
  (`crates/verbatim-speech/src/manager.rs`, `drop_expired`), a critical
  bug (Dickson, 2026-10-29).
- The menu-item clause of the culling of expired focus speech judged for
  every waiting utterance, as NVDA does, where Verbatim judges it only
  for the focus just left (the focus pipeline audit).
- The audio step of the communication design
  ([communication](design/communication.md), section 11; Dickson,
  2026-10-08): the audio stream stays awake after speech, writing silence,
  for a configurable period, 30 seconds by default as in NVDA, because
  Bluetooth, USB, HDMI and virtual-machine endpoints otherwise clip or
  delay the next speech; cancelling keeps Stop and Reset, as NVDA does,
  since shared mode has no other way to discard queued audio; the engine
  period trial; and the earcon re-mix, where speech that starts while an
  earcon plays is mixed into audio not yet played, re-mixing what was
  already handed to the device, so an indication's sound and its speech
  start together and cancelling an utterance cancels both. Speech now
  waits behind the roughly 40 ms of earcon already queued, which delays
  every start-up's first announcement. The latency ledger stops counting
  the tail of earlier speech as "mixer and device", which made
  back-to-back utterances look slow there. Each is measured with the
  device stages of the communication design's section 10.1.
- Passing runs log 1 to 10 warnings "the device ran dry in the middle of
  an utterance: an audible gap" from `verbatim::audio` (debug build); look
  at them with the audio step.
- The audio device awake time setting, with the awake period above.
- What Verbatim does for a user whose first OneCore activation after logon takes longer than the synthesizer host's ten-second hang bound is open (Dickson, 2026-10-21).

## M6.4 — Terminal residuals

What remains of the terminal redesign (Dickson, 2026-10-29).

Built from [the terminal model](design/terminal-model.md) (accepted by
Dickson on 2026-10-25), which states what is read, the rules and the
limits; each part is one step in the sense of `REVIEW.md`, with nothing
pushed until the last. The model in `verbatim-text`, its move into Core's
reducer, "Report status bar" and moving by command are built. What
remains, in order:

- Pi's regular mode in the moving-by-command and "Report status bar"
  scenarios, once Pi is installed on the test machine.
- Recordings for the conformance corpus, after checking that the NVDA pin
  equals the installed build: Pi's regular mode resized, set aside until
  the corpus can record a resize (Dickson, 2026-10-25); fzf, once Dickson
  installs it. htop is dropped (Dickson, 2026-10-25).
- End-to-end scenarios: the existing terminal scenarios brought back to
  passing; then cargo's build bar below its output; the landing rule's
  keys; Control+W, Control+U and Control+K; tab completion; suggestions; a
  completion list; menu completion; history search; output while typing;
  pasting; slow echo; a spinner with a message; several progress lines;
  resizing; a list shown by colour; a checkbox list; a refreshing
  full-screen script; fzf-like filtering; the review cursor kept through a
  footer redraw and moved with its text through a scroll; output just
  after the focus with an interactive shell; the console host's real
  password prompt; and Pi in both modes. Then, as decided on 2026-10-24,
  Verbatim's own streaming program (`verbatim-stream-test`) at up to 60
  frames a second in each way programs stream, with a footer, a header and
  a middle status row, expected to speak the streamed text once, in order,
  with no word split, whatever the reads' timing.
- The measurements after, in [performance](performance.md), for typing, a
  20,000-line flood, a Pi stream at its recorded pace, a clock ticking
  once a second, and a full-screen program refreshing rows, against the
  baseline of processor time, wake-ups and change-to-speech time taken on
  2026-10-23 on the build before the reading changes (runs under
  `target/perf/terminal-baseline-*`; the memory baseline in
  [performance](performance.md), "Memory use").
- [The terminals parity entry](parity/terminals.md), the crates'
  documents and these entries brought in line with the model.
- The whole terminal suite, `cargo xtask ci`, and the push.

Related items, each to be settled within the redesign or after it:

- Provisional, awaiting Dickson (2026-10-11): the console host raises no
  text change for a write that leaves the cursor where it was, as a full
  history scrolling whole lines may (the model's "The terminals"); if a
  measurement on the installed build confirms it, follow NVDA, which has
  the same gap.
- An unsettled terminal read with nothing queued behind it is never
  retried; `conhost_leave_flood` once heard no "blank" for an empty new
  console window, its first read giving nothing within 15 seconds.
- Windows Terminal sometimes raises no TextChanged event for a typed
  character, only caret moves, so about 5% of runs do not echo it; settle
  whether Windows Terminal or UIA loses the event, with about 60 runs and
  a second, independent UIA client listening. The model's reading on
  caret events (its "Reading rules", rule 1) comes back with the outpost
  and Core step (Dickson, 2026-10-17).
- `windows_terminal_two_windows` may say "<title> terminal" two or three
  times, from Windows Terminal's second runtime identity (Dickson,
  2026-10-19; [UIA faults](uia-bugs.md), "Windows Terminal raises focus
  from a second runtime identity"); the rule is M5's.
- The console host's say-all repeats one row: its text pattern clamps an
  expansion below the last row with text back onto that row, and nothing
  checks that the expanded line contains its point. Proposed fix: take a
  unit that does not contain its point as the end of the text, one
  comparison inside the existing remote program, judged by a fake-source
  unit test, a mockapp provider whose expansion clamps, and a console host
  scenario step from a fresh NVDA capture.
- `conhost_full_screen` races its program: a read taken before
  `altscreen.ps1` parks the cursor speaks "row N changed", on the base
  build too.
- A lost read was seen to ask twice for the whole document, where one
  whole-document read should answer it.
- "hellook1" is heard in the recorded hand streams.
- Windows Terminal's launch focus sometimes speaks "... terminal" without
  "... window", and the desktop's announcement is sometimes cancelled
  (the `*_up_typing`, `*_typing` and `*_commands` scenarios).
- `conhost_typing` missed a typed "o" in 2 of 6 runs on the base build.
- The reader's fix for a caret ahead of the typing covers only a line
  showing none of the pending typing: with "a" and "b" typed, a line
  showing "a" with the caret one cell ahead gives Gained "a" and then
  Changed "ab".
- Enter that erases a grey suggestion speaks the input line again.
- Up Arrow and then "e" typed before the recalled line is read gives Echo
  "e" and then Gained "cho one".
- Backspace erasing "o", Control+Backspace erasing "tw", and tab
  completion have no exact reader tests.
- A trimmed caret line taller than the kept screen could miscount the
  caret's column along its line.
- The reader ignores the colours of a colours re-read that comes late.
- The outpost takes a read's `ended_ms` after converting its text from
  UTF-16, so the conversion's time counts as part of the read.
- A review command's history read is placed with gaps in its checks: its
  answer carries no `from_start`, so a start the outpost refused during
  the read is placed as if counted from the read it names; an
  `Unanswered` history read is placed as no rows above the start; and a
  whole-document history read is placed by the end of the memory's rows
  rather than by a start, so more than a screen of unshown output printed
  during the round trip shifts every placed row (ReviewTop then
  ReviewNextLine read the wrong line). The outpost could count a whole
  read's rows from its newest start too, so Core places it by start.

Follow-ups:

- Root-cause, from the GitHub run logs, the hosted end-to-end `windows_terminal_two_windows` and `windows_terminal_leave_flood` failures on main before 2026-10-15; neither reproduces here.
- Repeat `line_key_flood` on GitHub's runner, where it once read a half-written line (deferred, Dickson, 2026-10-21).
- The cleanup of a stuck flood console, which once outlived its scenario until the agent was force-killed, is not tested.
- An outpost crash inside UIA's saved-event delivery (`ProtobufHelper::GetCacheResponse` reading a null element reference on a UIA worker thread): with the focus pipeline's section 17 built, `conhost_leave_flood`'s console host outpost failed fast again; read its full dump, kept outside the repository, and make section 17's fifty-run check.
- A console window read through MSAA is "pane" where NVDA gives its client object the legacy console overlay with the terminal role (`nvda/source/NVDAObjects/IAccessible/__init__.py`, lines 731 to 733, and `IAccessible/winConsole.py`; deferred, Dickson, 2026-10-21).
- A console whose provider's formatting check gets no answer is probed and checked twice in one arbitration entry, through the `None` branch of `decide` in `crates/verbatim-outpost/src/arbitration.rs`.

Exit: Terminal sessions are solid; terminal flood E2E shows bounded
latency and no hang.

## M6.5 — Text residuals

The text work left after M4's caret and selection rework, gathered in
one milestone (Dickson, 2026-10-29).

Formatting and the rich edit control:

- A rich edit control read through its own object model
  (`ITextDocument`), as NVDA reads a rich edit control whenever it can
  get that model (`ITextDocumentTextInfo`,
  `nvda/source/NVDAObjects/window/edit.py` 1041 to 1045): its own
  paragraphs, so that Control+Up and Control+Down Arrow speak the
  paragraph rather than the wrapped line, its own sentences for Alt+Up
  and Alt+Down and say-all, its embedded objects named, its paragraph
  alignment and its links. Verbatim reads a rich edit control through
  window messages, its paragraph unit being its line as shown
  (`docs/parity/text.md`, "Rich edit paragraphs").
- Formatting in classic edit controls, plain and rich, as NVDA reads it;
  they read none today.
- Formatting in review commands and say-all, which read none today
  (`docs/crates/verbatim-core.md`, "Formatting").

Text and selection:

- Settle where NVDA's say-all starts from NVDA's speech log before any say-all expectation changes: in every capture of 2026-10-23 NVDA's recorded speech began at the line after the caret's, in Notepad and in the text box, which is either this NVDA build or the capture add-on missing the first chunk.
- Notepad's Find box differs from NVDA's capture: Control+F says Notepad's notification "Press F6 to go to new notification" first and "Find edit" and "selected beta" as two utterances, where NVDA says "Popup window", then "Find edit selected beta" as one, then the notification; settle it with a capture that does not depend on the text Notepad remembers in the box.
- The order of a selection change's parts: NVDA speaks every text selected, then every text unselected (`speakSelectionChange`, `nvda/source/speech/speech.py` 1367 to 1417), where Verbatim says them in text order, so a replaced selection and one crossing its anchor are spoken the other way round; no capture has the replaced selection yet.
- NVDA's classic edit control drops a caret event whose selection offsets equal the last one's (`NVDAObjects/window/edit.py` 1068 to 1075); Verbatim reads each caret event (`docs/parity/text.md`, "Not yet built").
- The remote caret program could leave out its two comparisons of the old and new selections' ends when the old selection is known at build time to hold nothing (`previous_collapsed`, `crates/verbatim-uia-rops/src/caret.rs`), since that case is a whole change already.
- The caret read made with a focus chooses remote or classic reading by the previous focus's window, since the outpost's window tracking is updated after the focus is published; only the read's cost differs.

Typing and keys:

- Verbatim echoes a typed character from its keyboard hook, before the application handles the key, where NVDA echoes from the application's typed-character event; a letter typed within about 40 ms of Tab is not echoed (Dickson, 2026-10-10, for a later milestone).
- Key holding's gaps: no test or scenario of a word read in a rich edit control; `Modifiers::after` keeps one flag for left and right Shift, Control and Alt; a held key whose release is never seen stays in `held_downs`; and `call_counts.rs` pins the read before a held key by copying the production code's arguments rather than calling it.

Exit: each entry is matched (verified) by a scenario or a test, with a
declared deviation where Dickson chose one.

## M7 — Browse mode, browsers, and the injection helper

- Document projection / incremental virtual buffer over the normalized tree;
  quick-nav keys, elements list (presented through the M3 list dialog),
  switching between focus and browse modes. Where the buffer lives is
  decided here: in Core, or in the outpost or the injection helper with
  Core querying it, as NVDA keeps its buffers inside the browser's
  process. Either way Core's state stays cheap to clone (agreed
  2026-10-06): a buffer held in Core keeps its text in a rope and its
  nodes in a persistent map, both sharing memory when cloned, so the
  flight recorder's checkpoints stay affordable.
- IA2 lands here, deferred from M3 because browsers are what motivate it:
  interface acquisition (`IServiceProvider::QueryService` from the
  WinEvent's `IAccessible` to `IAccessible2` and the IA2 text, hypertext,
  and relation interfaces), the proxy/stub marshaling story from Rust, and
  IA2 roles and states preferred over MSAA in the normalized mapping.
  `mockapp`'s MSAA mode grows an IA2 answering path so the client stack is
  testable cross-process without a browser.
- Firefox and Chromium (Edge/Chrome) via IA2 as the primary source, with a
  UIA comparison where relevant.
- The D2 injection helper lands here, staged inside the milestone:
  out-of-process IA2 first, proving correctness, then the in-process helper
  for performance — IA2 call batching and virtual-buffer acceleration —
  including the x64/ARM64EC/x86 helper matrix and antivirus/signing
  considerations. NVDA has already proven that in-process access is what
  makes browsing fast enough, so this is scheduled work rather than a
  measure-first gate; the fixed-page-corpus measurement against NVDA
  remains as this milestone's exit verification.
- Screen review as a spatial projection of the normalized tree (D11):
  visible nodes ordered by bounding rectangle, grouped into visual lines,
  walked by the review cursor by line, word, and character. Extent-backed
  wherever a text interface exists — UIA TextPattern bounding rectangles
  and IA2 character extents give exact per-character geometry, batched
  through the new helper — with rectangle interpolation as the fallback for
  name-plus-rectangle elements. OCR joins as a second text source in M8 and
  the display model as a third in M14; the review commands and cursor stay
  the same throughout, only the source improves underneath.
- Typed-character echo of composed text from the D2 injection helper, as
  NVDA echoes what the application receives: Windows' text-service
  keyboards outside Chinese, Japanese and Korean (Vietnamese Telex and
  VNI, the Indic Phonetic keyboards) compose characters the keyboard hook
  cannot see. Until then Verbatim stays silent while such a keyboard is
  active rather than echoing raw keys (decided 2026-10-08).
- IAccessible2 unique ids as identities: NVDA uses them to recognize an
  IA2 object again when it is named by an event, so a state change on an
  IA2 focus ancestor can be spoken. Plain MSAA objects named by events are
  oleacc wrappers whose COM identity never matches an object reached
  through `accParent` (found 2026-10-08, `docs/crates/verbatim-ia2.md`,
  "Identity"), so without such ids neither NVDA nor Verbatim speaks those.
- Interaction-before-full-render E2E on a very large page.
- Scan-mode generalization: the same projection over an ordinary app.
- Start-menu search results: the results are a Chromium (WebView2)
  document inside the search host, which NVDA reads through IA2
  (`appModules/searchui.py`, `StartChromiumObj`); once IA2 lands, a
  Start-menu search scenario types a query
  and arrows through the results (NVDA's reading, captured on 2026-10-06: "Notepad, App, Press
  right to view options 1 of 5" after the first letter, each result with
  its position as the arrows move).
- From NVDA's changes up to 663e4b679 (reviewed 2026-10-06): Alt+Up and
  Alt+Down collapse or expand the control at the caret, or move by
  sentence, depending on the control (`browseMode.py`); focusing a
  `role=application` element announces the labels of the regions and
  groupings it enters; a native selection mode setting; quick navigation
  to clickable elements; a history in the Find dialog; virtual buffer
  attributes whose names are not valid XML are dropped.

Exit: real browsing works day-to-day; large-page E2E passes; corpus
performance/parity report vs NVDA written, at (or consciously accepted
near) parity; screen review reads a modern app's screen correctly.

## M8 — Breadth: speech configurability, profiles, overlays, OCR

- Automatic language switching, as NVDA does it: the language of each
  run of text (UIA's Culture per format run, IA2 text attributes) carried
  through the text reads, Core and speech, and each run spoken in its
  language's voice by every synthesizer, with a setting to turn it off.
  Today the language is read but used only for word segmentation, and
  OneCore's SSML always uses the current voice's language (language audit,
  2026-10-08). Terminal output switches language only where the terminal
  reports a language per line (decided 2026-10-07).
- Input help mode; the gesture-remapping configuration GUI; full
  pronunciation/symbol dictionaries and their configuration UI (the
  data-driven infrastructure exists from M3/M4). The built-in
  dictionary's letter-to-digit rule, a space wherever a digit follows a
  letter, is built with them, after checking whether eSpeak NG and
  OneCore still need it ([speech and audio parity](parity/speech-audio.md),
  "Letter-to-digit splitting"; Dickson, 2026-10-15). With punctuation and
  symbol levels in place, decide how terminal output's symbol-only lines
  (vim's "~" rows, box-drawing rules) are spoken: M4 speaks them as they
  are, and they should then follow the user's symbol level, as NVDA's
  are silent at its default level (Dickson, 2026-10-08). The compare
  check once symbol levels exist: at
  `windows_terminal_commands`'s first prompt NVDA says "ready greater"
  at its default symbol level ("some"), where Verbatim leaves the ">"
  out (Dickson, 2026-10-21).
- Configuration profiles (manual and triggered); localized UI shipped in at
  least two languages as proof.
- Focus highlight (DirectComposition overlay) and screen curtain (R6 check);
  overlay component designed to host a future magnifier.
- OCR capability (Windows.Media.Ocr) exposed to extensions; synthetic-subtree
  review of an image/window; OCR becomes screen review's second text source,
  for windows whose pixels contain text that no API exposes.
- Installer/updater skeleton.
- From NVDA's changes up to 663e4b679 (reviewed 2026-10-06): copying the
  last spoken text to the clipboard (NVDA+Control+X) and a controller
  call reporting whether speech is in progress; dictionary rules written
  as zero-width lookarounds, which avoid catastrophic backtracking (NVDA's
  rewrite of its camel-case and word-to-digit rules in `builtin.dic`);
  OCR captured through Windows Graphics Capture; menu item locations in
  32-bit applications converted from the 96-DPI space the OLEACC menu
  proxy reports when the client's bitness differs (needed once locations
  are used, by the focus highlight or the mouse).

Exit: curtain + highlight E2E.

## M8.1 — UIAccess: elevated windows, UAC and sign-in

Reading elevated console windows, UAC prompts and the lock screen or
sign-in needs the UIAccess permission; this milestone builds it and reads
each of them end to end.

- Secure-desktop instance (`--secure`), AT registration, UIAccess/test-signing
  story in the VM (R5).
- Remove the keyboard hook's guard that holds no caret key while a
  window of higher integrity is in front
  (Core's `SrState::hook_foreground`, from the integrity level the focus
  listener reports for the window in front, published in the hook's
  `ForegroundView`): with UIAccess, keys Verbatim
  sends on reach elevated windows, so they are held there too
  ([holding caret keys](design/key-holding.md), "Elevated windows";
  Dickson, 2026-10-22).
- The secure desktop and the lock screen wait for UIAccess support,
  without which Verbatim cannot read the secure desktop (Dickson,
  2026-10-12).
- An elevated console window is named "Administrator: <title>" or just
  "<title>" depending on when it is first read (`conhost_raised_flood` on
  GitHub's elevated runner). Capture what NVDA says for
  an elevated console window, which needs an elevation prompt Dickson
  approves, then make Verbatim read one name consistently.

Exit: a UAC prompt, an elevated console, and the sign-in screen are read
in the VM.

## M9 — Logging and log viewer

Proper user-facing observability, distinct from (and built on) the
developer-facing flight recorder and tracing spans. M5 brings forward the
minimal pieces its modules need: the in-memory ring, a log viewer window
and the Verbatim+F1 focus report; this milestone finishes the story.

- User-facing log levels and categories; logging to file on top of the
  ring; the existing tracing spans, collected stderr, and panic dumps
  absorbed into one coherent story.
- The log viewer window grows from M5's minimal viewer into one readable
  with Verbatim at every level and category — the viewer is its own
  dogfooding test.
- The commands NVDA users expect around it: open the log viewer, report the
  most recent error, cycle the log level at runtime.

Exit: a user can reproduce a bug, open the log viewer, and read what
happened, without touching developer tooling.

## M10 — Extension console and scripted modules

The equivalent of NVDA's Python console for extension development, and
the scripted module kind, both deferred from M5 (Dickson, 2026-10-22)
because a script engine compiled to Wasm costs every outpost that hosts
it. Until then M5's call console in `verbatim-inspect`, which calls a
module's hooks and tests rules against a live Verbatim, is the smoke test.

Python is an obvious fit for NVDA; with Wasm extensions the console is
instead an interactive interpreter that is itself an extension: an
interpreter compiled to Wasm, granted broad capabilities, evaluating
against exactly the `verbatim:ext` WIT API every extension uses. No
privileged side-channel API exists — if the console can do it, an
extension can — so the console doubles as a standing test that the API is
ergonomic enough for exploratory work. Convenience aliases in the NVDA
console style (the focused object, the navigator object) are pre-bound
bindings over the same calls, never separate host functions. A scripted
module is the same interpreter loaded with a module's script, with
TypeScript by type erasure, and sits in a module folder exactly as a
component does.

Design questions settled inside this milestone: which interpreter (QuickJS,
RustPython, or similar, compiled to a component), how epoch preemption
interacts with long-running evaluations, and whether the console UI lives
in the Verbatim GUI, in `verbatim-inspect`, or both.

Exit: from the console, inspect the focused object, walk its tree, speak,
and bind a gesture — against a live Verbatim, without restarting anything.

## M11 — Audio formatting: earcons and voice styling

The payoff for D12's structured utterances: presentation themes that map
semantic spans to sound, in the tradition of Emacspeak's audio formatting
and the audio-themes family of NVDA add-ons.

- A theme maps span semantics to presentation: a role can become an earcon
  plus shorter speech (a slider sound and "pitch 50" instead of "Pitch
  rate slider 50"); formatting attributes on text runs can become sounds
  (a spelling or syntax error under the cursor plays a sound rather than
  being spoken); capitals, quotes, and emphasis can become pitch or voice
  changes.
- The default theme, built in M4, matches NVDA's defaults: everything is
  spoken as NVDA speaks it, and sounds play where NVDA plays them by
  default, plus Verbatim's own cues and the error sound for a spelling
  error (decided 2026-10-07). A theme without sounds is added when
  braille arrives. Switching themes is a runtime configuration change, no
  restart.
- Themes are data, and eventually extension-provided packages — giving the
  porting track a consumer in the audio-themes add-on family.

Exit: an earcon theme ships alongside the default theme; a scripted E2E
hears the slider earcon and the spelling-error sound; the default
theme's output is unchanged from before M11.

## M12 — Remote support

- Pairing/auth UX on the control plane; speech mirroring out, input in;
  secure-desktop and permission rules applied to remote sessions.

Exit: control a second Verbatim instance (between the VM and the host)
end-to-end.

## M13 — Java Access Bridge

JAB is a committed backend, deliberately last among the accessibility APIs
(D1): far fewer apps need it than UIA and MSAA/IA2.

- `verbatim-jab` client stack: the WindowsAccessBridge-64 C API and its
  event callbacks, running on outpost threads like the other backends,
  mapped into the normalized model behind the same arbitration.
- A small Java Swing fixture app joins `mockapp` duty for provider-level
  tests; E2E against a current Java IDE or LibreOffice-adjacent Java app.
- Unblocks porting-track Tier E (javaw, Eclipse).

Exit: a mainstream Swing application is readable and navigable end-to-end.

## M14 — Display model (gated)

A GDI display model in the NVDA tradition, deliberately last among the
text sources (D11), and opened by an explicit re-triage gate: test the
then-current PuTTY, SecureCRT, and Tera Term (and any other app that has
motivated this milestone by then) against stock Verbatim plus OCR, and
build only if the display model still earns its maintenance cost. By this
point everything it needs already exists, which is what keeps the
milestone cheap: the M7 injection helper is the delivery vehicle (the
display model is its second in-process client), synthetic nodes make its
output first-class model content, and the M4 diff announcer and flood
policy handle live terminal output.

- GDI text-output hooks (`ExtTextOutW` and family) in the injection helper;
  a per-window text model (chunk rectangles, baseline ordering,
  invalidation on redraw).
- A live-text diff source over that model — the `DisplayModelLiveText`
  equivalent — feeding the same announcement pipeline as Windows Terminal.
- Screen review gains the display model as its third, pixel-faithful text
  source for GDI apps; text-under-mouse works in apps with no text API.
- Unblocks the porting track's legacy terminal clients (PuTTY, SecureCRT,
  Tera Term).

Exit: the re-triage decision is recorded; if built, a PuTTY session is
readable with live output announcement and the existing screen-review
commands, with no fidelity regression anywhere else.

## M15 — Braille (deliberately last, D7)

- liblouis integration; `BrailleDisplay` trait implementations for common
  displays; braille viewer (on-screen virtual display) so development and
  E2E tests need no hardware; routing keys, cursor tethering, speech-braille
  sync via existing index marks.

Exit: braille viewer E2E scenarios pass; at least one hardware display
verified when hardware is available.

## The app-module porting track (runs from M5 onward)

Goal: eventually port all of NVDA's built-in app modules
(`nvda/source/appModules`, roughly 80) except those for software that no
longer exists or is unmaintained — Skype, Lync, MSN Messenger, Outlook
Express, Windows Live Mail, Winamp, Instantbird, Miranda, Lotus
Notes/Symphony, the legacy EdgeHTML modules, and similar; each skip is a
recorded case-by-case call.

Method: triage before porting. Many NVDA modules exist to patch quirks of
NVDA's pipeline or of APIs at the time they were written; for each module,
first test the app against Verbatim's stock UIA/IA2 handling and port only
the behavior that is still needed. Every port doubles as a test of the
extension API (per the API-growth rule: no host-API additions without a
consumer).

Tiers, gated by the capabilities each module needs:

- **Tier A — immediately after M5** (needs only M5's surface: rules,
  event and node hooks, speech, commands): the utility modules beyond the
  Windows inbox applications, which are M5's own built-ins — Notepad++,
  Poedit, Spotify, foobar2000, Audacity, 1Password, basic Zoom and Teams
  behavior. Notepad++'s support, the
  first Scintilla editor, takes with it NVDA's per-class rule that a
  Scintilla editor's caret shows and hides are no caret event
  (`shouldAcceptShowHideCaretEvent`, `NVDAObjects/IAccessible/scintilla.py`,
  line 15, tested in `IAccessibleHandler/__init__.py`, lines 1039 to
  1055), since its caret blinks by them; until then a focused Scintilla
  editor tries to read its caret at each blink.
- **Lock screen and logon UI — after M8.1**: reading the sign-in screen
  needs UIAccess, which M8.1 builds.
- **Tier B — the terminal clients, in two stages**: Windows Terminal
  workflows are core M4 behavior, and mintty is triaged on its own after M4
  (its ConPTY integration may make it readable stock). PuTTY, SecureCRT,
  and Tera Term draw their screens through GDI with no accessibility API at
  all, so they are gated on the M14 display model — with ssh from Windows
  Terminal as the recommended interim answer.
- **Tier C — after M7 browse mode** (web-content-hosting apps): WebView2
  hosts, VS Code, WhatsApp, deeper Teams support, Thunderbird, Kindle and
  other readers.
- **Tier D — after the object-model bridge capability lands (planned around
  M8)**: Word, Excel, PowerPoint, Outlook, LibreOffice, Visual Studio.
  NVDA's Office modules rely heavily on COM object-model automation, which
  Wasm extensions cannot reach directly; this tier is gated on designing a
  capability-gated app object-model bridge in the WIT host API — the
  largest single API-growth item in the plan, and worth its own design pass
  for both surface and security. Word's support takes with it NVDA's
  rule that the system caret's events are no caret event on Word's UIA
  document, which Chinese input methods flood with them
  (`IAccessibleHandler/__init__.py`, lines 641 to 657); Verbatim handles
  them as a caret event on every UIA focus until then.
- **Tier E — after the JAB backend lands (M13)**: javaw and Eclipse.

## Ongoing tracks (every milestone)

- **Latency**: the end-to-end suite records each step's response time
  and asserts none. Enforcing the response-time budgets is decided but
  not yet built (Dickson, 2026-10-08, reaffirmed 2026-10-09): enforced by
  default in local runs, recorded but not enforced on GitHub's runners,
  and with them a failure when verbatim.exe's relay takes over 2 ms.
- Memory growth (Dickson, 2026-10-08): a scenario of
  about 25 seconds in every suite, ten iterations of a Windows Forms text
  box window opened, edited, reviewed and closed, and the settings dialog
  Tabbed through. After two warm-up iterations, the counts Verbatim
  reports through the control plane return to exactly the same values
  after every iteration, and each process's private memory grows from
  iteration 3 to 10 by no more than a small bound, enforced in local runs
  and recorded on GitHub's runners.
- A behaviour-based MSAA audit (Dickson, 2026-10-08):
  live NVDA captures against a matrix of standard controls, compared
  exactly with Verbatim.
- Medium integrity on GitHub's runner (Dickson, 2026-10-10): the e2e job
  tries running Verbatim and the test applications at medium integrity, as
  users run them, after measuring that it behaves with the runner's UAC
  disabled. The runner is elevated today (`runneradmin`), which is what
  puts "Administrator: " in console titles.
- **Localization**: no hardcoded user-visible strings, ever; pseudo-locale
  test from M1.
- **Flight recorder into regression tests**: every reproduced field/VM bug
  lands as a replay test.
- **Extension API growth**: only via porting real app modules/add-ons;
  each addition needs a consumer.
- **Continuous integration** (D3, amended 2026-09-02): GitHub-hosted
  Windows runners run the end-to-end suite runner-direct, silently, with
  recordings from the audio tee (D16); that is the whole of CI. The VM
  harness is the interactive loop. `.github/workflows/vm-smoke.yml`, which
  probed nested virtualization on larger runners, is to be retired.
- **Housekeeping**: Cargo warns that `proc-macro-error2` 2.0.1, which
  `i18n-embed-fl` depends on, will be rejected by a future Rust
  (`cargo report future-incompatibilities`: it re-exports the private
  `proc_macro` crate, E0365); move off it before the pinned toolchain
  reaches a Rust that rejects it.
