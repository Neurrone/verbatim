# Verbatim Roadmap

Milestones are scoped by _risk retired_, not calendar time. Each has explicit
exit criteria so progress is testable. Ongoing tracks (localization,
observability, latency budgets) start early and run through every milestone
rather than being milestones themselves.

## Status

M0 (foundations), M1 (self-voicing prototype), M2 (test harness and
VM), and M3 (desktop usability core) are complete. Their full scope
and exit-criteria evidence are archived in
[Completed milestones](roadmap-done.md). M4 is in progress; the
parts of it that have landed are listed there too, under "M4, completed
so far".

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

Exit: Notepad editing and Terminal sessions are solid; terminal flood E2E
shows bounded latency and no hang.

M4 ends when the items in the two groups below land; the rest of the
open work found during M4 is scheduled in the milestones that follow. A
Deferred list closes the section with items that are not M4's.

What has landed is in [Completed milestones](roadmap-done.md), "M4,
completed so far". The open work was planned as three groups, each one
session (Dickson, 2026-10-15). The first, the focus pipeline with its
File Explorer evidence pass, landed on 2026-10-17, and its follow-ups are
in M4.1. Two groups remain: the terminal redesign with the key-holding
question, and the audio step; within a group, the items are in the order they are done
(Dickson, 2026-10-10 and 2026-10-13). Each item is one change,
checked with `cargo xtask ci` and its related scenarios ten times, with the
full suite where every scenario is affected. A change in what is heard goes
to Dickson first; while he is away, it follows NVDA where NVDA is clear,
recorded as provisional.

### The terminal redesign

Built from [terminal reading](design/terminal-reading.md) (approved, its
open questions 1 to 12 answered as recommended) in its small steps, after
its measurements, with the corrections of
[its review](design/terminal-reading-review.md).

- The redesign: one read per change as the unit, every other call only a
  hint, an original line diff, and each extra beyond NVDA's model decided
  separately. It removes ecf191c, restores a scenario with an interactive
  shell for output written just after a terminal takes the focus (with an
  NVDA capture), and restores the console host's real password prompt. It
  includes a test bounding the diff's work on a 9,000-line overflow and a
  2,000-line flood, asserting exact counts. Lists a program redraws in two
  writes are decided here, with a scenario that controls when each piece
  is written (Dickson, 2026-10-10).
- The review's corrections: key attribution uses each read's own start
  and end times, carried in the outpost's message on a monotonic clock
  shared with the key's press time, not Core's receipt time; the
  whole-document fallback gets its own alignment bound; and the NVDA
  details the review lists.
- Provisional, awaiting Dickson (2026-10-11): if measurement 5 confirms
  the console host raises no text change while a full history scrolls
  whole lines, follow NVDA, which has the same gap; a flood of identical
  lines cannot be counted whenever the history is full, documented as a
  limitation.
- Bug fix: the caret read with a terminal's text is stamped with the
  change event's time, not the read's, so Core can take a caret already
  showing a key's effect as the caret before it (a likely cause of
  `conhost_typing` step 5 failing).
- Bug fix: Windows Terminal faults on a `FindText` that finds nothing
  (an access violation on its UI thread, seen as `RPC_E_SERVERFAULT`); the
  redesign drops `FindText` (the design's corrections of 2026-10-11).
- An unsettled terminal read with nothing queued behind it is never
  retried; and `conhost_leave_flood` once never heard "blank" for an empty
  new console window, its first read giving nothing within 15 seconds.
- A typed character Windows Terminal does not echo in about 5% of runs:
  Windows Terminal sometimes raises no TextChanged event for it, only
  caret moves, and Verbatim reads the screen only on a text change (NVDA
  has the same dependency and would say the character late, with the next
  change). First settle whether Windows Terminal or UIA loses the event,
  with about 60 runs and a second, independent UIA client listening; the
  proposed fix is that the outpost treats a caret row that differs from
  its last screen read as a change (Dickson, 2026-10-17: part of this
  redesign).

- A throwaway prototype of holding keys, as NVDA holds a key with
  a script while it reads the caret, measured, with a written analysis;
  holding keys is Dickson's decision. Its first case is not chosen yet:
  Dickson picks it. `notepad_say_all` is no longer a candidate, because
  its failure was not a key reaching the application before say-all's
  caret move: neither Core nor the outpost took that move as the caret,
  so the key's watch compared against the line before, and both now do.
  The prototype also measures what holding a key costs and how often a
  key's effect is misjudged, the baseline the work order of 2026-10-10
  asked for.
- The caret problems that share one cause, decided with the key-holding
  question rather than fixed one by one (Dickson, 2026-10-17). NVDA keeps
  no copy of the caret: its caret scripts read the caret from the
  application when the key is pressed and wait for it to move from there.
  Verbatim judges a key from Core's and the outpost's copies of the caret,
  kept by events, so any caret change reported after the key leaves them
  stale. Known cases: select then copy (Verbatim+F10) moves the caret but
  neither Core nor the outpost records where, so a caret key pressed
  before the application's report is judged from the caret before the
  selection (Right speaks the character at the selection's end, Shift+Right
  says the selection again; no test covers it); a say-all caret move the
  application refuses still becomes Core's caret; and a selection reported
  after the next key (Shift+Right followed at once by Right) leaves Core's
  previous selection stale. The fix proposed for the first, deferred: the
  outpost's reply to a select carries the caret it read after selecting,
  and Core takes it as its caret.

### The audio step

Decided by Dickson and not yet built. The reasoning is in
[the phase 6 design](design/phase6-design.md), in the dated section named
with the item.

- The audio step of the communication design
  ([communication](design/communication.md), section 11; "Audio start-up
  latency and scheduling", 2026-10-08): the audio stream stays awake after
  speech, writing silence, for a configurable period, 30 seconds by default
  as in NVDA, and cancelling keeps Stop and Reset; the engine period trial;
  and the earcon re-mix, where speech that starts while an earcon plays is
  mixed into audio not yet played, re-mixing what was already handed to the
  device, so an indication's sound and its speech start together and
  cancelling an utterance cancels both. The latency ledger stops counting
  the tail of earlier speech as "mixer and device". Each is measured with
  the device stages of the communication design's section 10.1.

### Deferred

- Review keys straight to the reducer
  ([communication](design/communication.md), section 11) is reconsidered
  when the command set is settled (Dickson, 2026-10-14).
- For a later milestone (Dickson, 2026-10-10): Verbatim echoes a typed
  character from its keyboard hook, before the application handles the
  key, where NVDA echoes from the application's typed-character event; a
  letter typed within about 40 ms of Tab is not echoed.

## M4.1 — Stability

Verbatim stops crashing and stalling, and the suite stops failing by timing.

- The outpost crash inside UI Automation's saved-event delivery
  ([the phase 6 design](design/phase6-design.md), "Console windows as one
  application"): section 17 of the focus-pipeline design is built
  (152f09a), but `conhost_leave_flood`'s console host outpost failed fast
  again on 2026-10-12. Read its full dump, kept outside the repository,
  and make section 17's fifty-run check.
- Verbatim's own outpost once left a fact in its intake for 6.37 seconds
  (2026-10-11); reproduce it with the intake and menu logs of f9d067d.
- MSAA events lost while the listener resumes
  ([the phase 6 design](design/phase6-design.md), "MSAA events in each
  outpost's own hook"): an event raised between a ready outpost's end and
  the listener resuming is lost; a recommendation is recorded there.
- An audit of every end-to-end scenario for timing races (2026-10-11),
  each race fixed by waiting for evidence, not a longer bound; and the
  agent's bring-forward, which always takes its slowest path (once 1.4
  seconds), its cause open.
- Scheduled for later (Dickson, 2026-10-11, from the communication
  design): the mixer's retry after a failed audio device open is a timed
  deadline, to be replaced by waiting for evidence such as a device
  notification; and the keyboard hook drops a gesture when the router's
  channel is full (`try_send` in `crates/verbatim-input-windows/src/lib.rs`),
  which must never lose a key.
- `CoCancelCall` for MSAA calls, a recommendation in the phase 6 design's
  "Time limits that hold".
- The audits' items 8 to 11 (deferred by Dickson, 2026-10-12, until after
  the queue): one duplicate test and one acceptance test per focus, as
  NVDA makes them; foreground events in the focus run as NVDA's `pumpAll`
  takes them; the events NVDA registers desktop-wide that Verbatim lacks
  (live regions, selection added and removed, progress values, window
  opened, layout invalidated, system alerts, drag and drop); and the
  smaller ones (the MSAA caret in UIA windows, the staging windows' focus,
  live reads of the focus's property events, NVDA's Windows Forms and UWP
  combo box rules, the controlled selection's route).
- The OneCore synthesizer host once failed to become ready within 10
  seconds on GitHub's runner, where it is otherwise ready in about a
  second. Its start is logged step by step (3baec60): if it recurs,
  investigate from that log, which shows the step where it stopped;
  otherwise drop this item when M4.1 closes.

- GitHub's CI on main had failed on the two pushes before 2026-10-15
  (runs 38271366442 and 38544999189), in two places that this machine
  never runs: the ARM64 job's unit test
  `the_codeview_record_read_is_the_one_dbghelp_reports` in
  `verbatim-trace-waits` (`server.rs`, an equality assertion), and the
  hosted end-to-end job, where `windows_terminal_two_windows` and
  `windows_terminal_leave_flood` failed beside the known
  `explorer_folder_window` placeholder (80 of 83 passed). Root-cause each
  from the run logs (`gh run view <id> --log-failed`) rather than from a
  local run, since neither reproduces here.

- The focus pipeline's follow-ups (2026-10-17):
  - `conhost_two_windows` fails about 1 run in 20, its cause found and
    its fix awaiting Dickson's decision. `UiaHasServerSideProvider`,
    called by a fresh client on a console window created moments earlier,
    answers no after about 3 seconds whenever any other process has a UIA
    event handler registered: experiment e38 in
    `target/e2e-evidence/launch-collision/collide/readme.md` saw it in 0 of
    110 trials with no other handler and up to 10 of 30 with one, and a
    single idle tooltip-opened handler is enough. The per-application lock
    cannot prevent it. A raw `SendMessageTimeout` probe sending
    `WM_GETOBJECT` with `UiaRootObjectId` answered correctly in all 360
    trials, and the lookup that followed returned the console host's own
    element. Replacing the probe (`crates/verbatim-uia/src/probe.rs` and
    its callers) replaces the call NVDA makes, and NVDA would get the same
    wrong answer (`nvda/source/UIAHandler/__init__.py`, lines 1279 to
    1287), so it waits for Dickson.
  - When a console window is read through MSAA, NVDA gives its client
    object the legacy console overlay with the terminal role
    (`nvda/source/NVDAObjects/IAccessible/__init__.py`, lines 731 to 733,
    and `IAccessible/winConsole.py`); Verbatim says "pane".
  - On a fresh GitHub runner, the first Verbatim start can say "Start
    toggle button not pressed" instead of "Desktop list": shortly after
    Verbatim's subscriptions are installed, the taskbar rehosts the Start
    button in a new PopupHost window, raising a UIA focus on a topmost
    window that reports keyboard focus (runs 39040893786 and 38980790680).
    NVDA's checks would accept that focus too. Dickson chooses between a
    throwaway warm-up start before the suite and an experiment to find
    what triggers the rehosting. The local cause of the same utterance,
    the agent's minimize activating the taskbar, is fixed (ace7858d).
  - Notepad's "app not responding" sound played once in `notepad_editing`
    on 2026-10-18, about 2 seconds after a right arrow, and the rerun
    passed; investigate if it recurs.
  - In File Explorer scenarios the listener's read once came 285
    milliseconds late. The listener logs how long its nearest-window
    lookup takes (f9fb32f7): locally most of the delay, about 230
    milliseconds, is the first lookup waiting on a busy File Explorer.
    Confirm the same on GitHub's runner.
  - Windows Terminal's terminal is sometimes said twice, or three times
    for a second window: in some launches Windows Terminal raises the
    terminal's focus under a second runtime id, the one starting 14,
    besides the usual one. The tests accept it as an optional utterance
    (`docs/testing.md`, "Optional utterances"). The cause is most likely
    a race in Windows Terminal's startup: it happens in about 4% of
    launches locally and 13 to 20% on GitHub's runner, the first such
    event arrives before any Verbatim client contacts the terminal, and
    it happens with spare outposts off too; NVDA receives it as well.
    The rule that recognizes it is deferred to a Windows Terminal app
    module in M5 (see M5, "Windows Terminal's second terminal
    identity"). Evidence is in
    `target/e2e-evidence/full-suite-2026-10-17/` and
    `target/e2e-evidence/full-suite-2026-10-17-spares-off/`.
  - An outpost's own ancestor and navigation walks into a frame host take
    no lock ([focus pipeline](design/focus-pipeline.md), section 20.9).
  - One UIA focus event reached the listener 5 seconds late in a console
    host run on 2026-10-17; watch for it again.
  - The trace tool's symbol lookup for ARM64X images, such as ntdll,
    needs a follow-up, which can be tested only on the ARM64 runner.
  - `line_key_flood` once read a half-written line on GitHub's runner;
    repeat it there.

Exit: a week of Dickson's daily use without a crash or a stall, and the
full suite ten times clean.

## M4.2 — Parity small items

Small differences from NVDA found during M4, each closed against a live
NVDA capture.

- NVDA's "Press twice to copy or reset the start marker", for a first
  press of Verbatim+F10 after a selection not yet copied, and the start
  marker cleared when a copy fails, where Verbatim keeps it (Dickson,
  2026-10-15; [review parity](parity/review.md), "Select then copy from
  the review cursor").
- The M4.2 compare runs capture NVDA's review bottom (Shift+Numpad 9) in
  the Windows Forms text box, to confirm or correct the source reading in
  [text parity](parity/text.md), "The review cursor's bottom in a
  standard edit control".
- Announce a selection change of the focus that no key and no Verbatim
  command made, such as one made with the mouse or by the application,
  as NVDA reports any selection change after its caret event (Dickson,
  2026-10-15; [text parity](parity/text.md), "TextInfo-equivalent
  layer"). The pieces are the outpost's caret watch and the
  selection-change reporting path built on 2026-10-15 (f964ba30).
- A foreground UIA progress bar that is not the focus is reported, as
  NVDA does (Dickson, 2026-10-10, as a separate fix); background progress
  bars are not yet reported (M4.3).
- Core speaks state changes such as "selected" for a focus that has
  expired.
- Toasts ([parity](parity/alerts.md), "Toasts"): Verbatim reads them only
  through an MSAA alert, NVDA through UIA's window-opened event. Capture a
  Windows 11 toast live with NVDA and with Verbatim; if Verbatim is silent,
  follow NVDA (a `Windows.UI.Core.CoreWindow` whose automation id contains
  `ToastView`, from a desktop-wide window-opened subscription in the
  listener), then adopt NVDA's rule dropping MSAA alert and show events
  from native UIA windows.
- A Settings search suggestions scenario with an NVDA capture. It also
  covers selecting inside a list the focus controls, which has no
  scenario and was not checked live after the cheaper reads of 2026-10-10.
- Text fields: on the classic text path `UiaText::caret_read`
  (`verbatim-outpost` `text/mod.rs` and `text/uia.rs`) answers as if it
  had read the caret, so the caret recheck added in 676d127 is skipped
  there; and the `(Some(Removal::Joined) | None, Some(deleted))` arm in
  Core's `editing.rs` has a case that cannot occur.
- A test that UIA text fields are read by parts (coherence review,
  2026-10-09).
- The review cursor on objects that no longer exist (the section of that
  name, 2026-10-08): review commands read live from the application
  rather than from Core's cache; a Gone answer for the navigator's node
  re-seeds it from the focus, or says "No navigator object" when the dead
  node is the focus; the outpost reports held MSAA nodes it forgets with
  their window; and an end-to-end test in both terminals that a reviewed
  line stays on its text as new output is written below it ("Revised").

- Three differences the first compare-mode runs found on 2026-10-15,
  each to be captured, decided and recorded as a parity entry: NVDA leaves
  out the second "spelling error" in a line with two misspellings, where
  Verbatim says it for each (`spelling_errors`, steps 2 and 11); NVDA says
  "ready greater" at a shell prompt where Verbatim leaves the symbol out
  (`windows_terminal_commands`, the first prompt; a symbol-level question
  first, as docs/nvda-transcript.md's settings note says); and the
  phonetic words after a character on some review gestures, to be checked
  again with NVDA's delayed character descriptions off.
- Two compare-mode limits, fixed when they get in the way: every
  comparison's start step differs because NVDA says nothing about a
  desktop already in front, so the report could treat the start step as
  not compared; and `terminal_overflow`'s go files (`phased-go`,
  `phased-go-2`) do not start their own step.

Exit: each entry is matched (verified) by a scenario, with a declared
deviation where Dickson chose one, checked with compare mode.

## M4.3 — Settings and indications

These are decided designs that daily use needs; the reasoning
is in [the phase 6 design](design/phase6-design.md), in the dated section
named with each item.

- The settings package ("NVDA's settings in Verbatim", 2026-10-08): the
  missing settings for behaviour Verbatim already has (capital pitch
  change, OneCore's pause after punctuation, the audio output device,
  sounds' volume following the voice, the audio device awake time,
  handling keys from other applications, the multiple key press timeout,
  the review cursor following focus, reporting tooltips, reporting
  notifications, the word segmentation standard, cancelling speech for
  expired focus events, trimming leading silence); dialog controls for
  the nine settings that exist only in `settings.toml`; and three small
  features (say "cap" before capitals, delayed character descriptions,
  the spelling-error sound while typing). The review cursor follows focus
  and the caret by default, each with NVDA's setting to turn it off.
  Spoken progress comes every 10 percent, as NVDA's does.
- Status cues ("Status cues settled", 2026-10-08): every indication in
  the theme's Events group (start and exit, error, application not
  responding, browse and focus mode, suggestions opened and closed,
  progress) plays through key presses; indications in every other group
  are content and are cut off. This is built-in behaviour defined by the
  group, not a setting.
- Sound-only announcements ("The review cursor in a terminal", the
  paragraph "Further", 2026-10-08): an announcement a theme reports by
  sound alone is queued, timed, measured and cancelled exactly like a
  spoken one; the start-up sound becomes one.
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

- Component host with epoch preemption; WIT `verbatim:ext` v0: event
  subscription, tree queries, speech output, gestures, config, storage.
  Ratify the D6 runtime choice here: verify wasmtime on Windows ARM64 and
  measure host-call overhead against the extension-hook deadlines before
  building further on it.
- App-module activation keyed to processes; hot reload; capability manifests.
- Dogfood: at least one first-party app module ported out of the core into
  an extension — the Explorer cosmetic fixes deferred from M3 are the
  designated first candidate — and a trivial Wasm synth proves the
  `verbatim:synth` world.
- Windows Terminal's second terminal identity, a candidate for a Windows
  Terminal app module ([design](design/windows-terminal-twin-focus.md)):
  Windows Terminal sometimes raises its terminal's focus under a second
  runtime id starting 14 besides the usual one starting 42, and the
  listener announces each as a new focus. The rule drops a 14 sender that
  is the twin of a current 42 focus; a 42 sender that is the twin of a
  current 14 focus is read and reported carrying the 14 id it replaces, so
  the outpost reuses that node and follows the 42 element, which it cannot
  find from the 14 id. Twins share a process, the class `TermControl`,
  control type, the XAML framework, nearest window, and the 42 id's parts
  after `[42, window]` as the 14 id's last parts; a failed
  `CompareElements` means no twin. Its tests also check that the terminal
  is followed afterwards and cover two panes in one window. Before
  implementing, collect evidence that the runtime id's last part is never
  reused (pane splits, pane moves, closing and reopening panes, new tabs).
  Until then the tests accept one repeated "terminal" as an optional
  utterance, which does not cover the late repeat ("window, terminal,
  blank, terminal, terminal, blank", a 42 to 14 to 42 flip seen on
  GitHub's slower runner), so that can still fail scenarios. Evidence:
  `target/e2e-evidence/github-38988118089/`,
  `target/e2e-evidence/github-39032739474/`, and
  `target/e2e-evidence/wt-twin-identity/` (the judge's runs and logs,
  `identity_judge.py`, `suffix.py`, and the scratch scenario's patch).

Exit: an app module can be edited and hot-reloaded without restarting
Verbatim; capability denial is enforced and tested. This milestone opens the
app-module porting track (see the section after M15), which then runs
continuously alongside every later milestone.

## M6 — Native synth host + Eloquence PoC

This milestone comes right after M5 and before browse mode so that
Eloquence can be delivered through the extension mechanism and
Verbatim can become Dickson's daily screen reader sooner (Dickson,
2026-10-15).

- Sandboxed native synth host process (AppContainer, job object, shared-memory
  PCM ring), arch-matched loading.
- Eloquence proof of concept meeting the same latency budget.

Exit: Eloquence speaks through Verbatim under sandbox; latency test green.

## M7 — Browse mode, browsers, and the injection helper

- Document projection / incremental virtual buffer over the normalized tree;
  quick-nav keys, elements list (presented through the M3 list dialog),
  switching between focus and browse modes.
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
  Start-menu search scenario (the earlier `start_menu_search`, deleted on
  2026-10-07 since it could not assert its speech exactly) types a query
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
  are silent at its default level (`docs/design/phase6-design.md`, "Terminal risks found by
  studying NVDA").
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
- The secure desktop and the lock screen wait for UIAccess support,
  without which Verbatim cannot read the secure desktop (Dickson,
  2026-10-12).
- An elevated console window is named "Administrator: <title>" or just
  "<title>" depending on when it is first read (`conhost_raised_flood` on
  GitHub's elevated runner, run 38031529579). Capture what NVDA says for
  an elevated console window, which needs an elevation prompt Dickson
  approves, then make Verbatim read one name consistently.

Exit: a UAC prompt, an elevated console, and the sign-in screen are read
in the VM.

## M9 — Logging and log viewer

Proper user-facing observability, distinct from (and built on) the
developer-facing flight recorder and tracing spans.

- User-facing log levels and categories; logging to file plus an in-memory
  ring; the existing tracing spans, collected stderr, and panic dumps
  absorbed into one coherent story.
- A log viewer window in Verbatim itself, readable with Verbatim — the
  viewer is its own dogfooding test.
- The commands NVDA users expect around it: open the log viewer, report the
  most recent error, cycle the log level at runtime.

Exit: a user can reproduce a bug, open the log viewer, and read what
happened, without touching developer tooling.

## M10 — Extension console

The equivalent of NVDA's Python console for extension development. Python
is an obvious fit for NVDA; with Wasm extensions the console is instead an
interactive interpreter that is itself an extension: an interpreter
compiled to Wasm, granted broad capabilities, evaluating against exactly
the `verbatim:ext` WIT API every extension uses. No privileged side-channel
API exists — if the console can do it, an extension can — so the console
doubles as a standing test that the API is ergonomic enough for
exploratory work. Convenience aliases in the NVDA console style (the
focused object, the navigator object) are pre-bound bindings over the same
calls, never separate host functions.

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

- **Tier A — immediately after M5** (needs only extension API v0: events,
  tree queries, speech, gestures): the Windows shell and utility modules —
  Explorer (starting with the cosmetic fixes deferred from M3), Settings,
  Task Manager, Calculator, search, Open With,
  Notepad, Notepad++, VS Code, Poedit, Spotify, foobar2000, Audacity,
  1Password, basic Zoom and Teams behavior.
- **Lock screen and logon UI — after M8.1**: reading the sign-in screen
  needs UIAccess, which M8.1 builds.
- **Tier B — the terminal clients, in two stages**: Windows Terminal
  workflows are core M4 behavior, and mintty is triaged on its own after M4
  (its ConPTY integration may make it readable stock). PuTTY, SecureCRT,
  and Tera Term draw their screens through GDI with no accessibility API at
  all, so they are gated on the M14 display model — with ssh from Windows
  Terminal as the recommended interim answer.
- **Tier C — after M7 browse mode** (web-content-hosting apps): WebView2
  hosts, WhatsApp, deeper Teams support, Thunderbird, Kindle and other
  readers.
- **Tier D — after the object-model bridge capability lands (planned around
  M8)**: Word, Excel, PowerPoint, Outlook, LibreOffice, Visual Studio.
  NVDA's Office modules rely heavily on COM object-model automation, which
  Wasm extensions cannot reach directly; this tier is gated on designing a
  capability-gated app object-model bridge in the WIT host API — the
  largest single API-growth item in the plan, and worth its own design pass
  for both surface and security.
- **Tier E — after the JAB backend lands (M13)**: javaw and Eclipse.

## Ongoing tracks (every milestone)

- **Latency**: the end-to-end suite records each step's response time
  from M2 on. Enforcing the budgets is decided but not yet built
  (Dickson, 2026-10-08, reaffirmed 2026-10-09): enforced by default in
  local runs, and recorded but not enforced on GitHub's runners (see
  the response-time budgets below).
- Response-time budgets (the afternoon decisions of 2026-10-08,
  reaffirmed 2026-10-09): enforced by default in local runs, recorded but
  not enforced on GitHub's runners; today the suite records them and
  asserts none. A failure when verbatim.exe's relay takes over 2 ms comes
  with them.
- Memory growth ("Memory growth tracking", 2026-10-08): a scenario of
  about 25 seconds in every suite, ten iterations of a Windows Forms text
  box window opened, edited, reviewed and closed, and the settings dialog
  Tabbed through. After two warm-up iterations, the counts Verbatim
  reports through the control plane return to exactly the same values
  after every iteration, and each process's private memory grows from
  iteration 3 to 10 by no more than a small bound.
- A behaviour-based MSAA audit (the afternoon decisions of 2026-10-08):
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
