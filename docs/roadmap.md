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
so far". M4.1 (stability) is complete and archived there; its open
follow-ups are under M4, "Follow-ups from M4.1".

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
File Explorer evidence pass, landed on 2026-10-17, and its open
follow-ups are under "Follow-ups from M4.1" below. Four groups remain: the terminal redesign, the audio step, the focus pipeline rework, and the caret and selection rework, the last two each in a session of its own (Dickson, 2026-10-25); within a group, the items are in the order they are done
(Dickson, 2026-10-10 and 2026-10-13). Each item is one change,
checked with `cargo xtask ci` and its related scenarios ten times, with the
full suite where every scenario is affected. A change in what is heard goes
to Dickson first; while he is away, it follows NVDA where NVDA is clear,
recorded as provisional.

### The terminal redesign

Built from [the terminal model](design/terminal-model.md) (accepted by
Dickson on 2026-10-25), each part one step in the sense of `REVIEW.md`,
with nothing pushed until the last. The model in `verbatim-text`, its move
into Core's reducer, "Report status bar" and moving by command are built.
What remains, in order:

- Moving by command's scenarios in Pi's regular mode, which wait until Pi
  is installed.
- Recordings for the conformance corpus, after checking that the NVDA pin
  equals the installed build for any capture: Pi's regular mode resized,
  set aside until the corpus can record a resize (Dickson, 2026-10-25);
  fzf, waiting until Dickson installs it. htop is dropped (Dickson,
  2026-10-25).
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
- The measurements after, against the baseline of processor time,
  wake-ups and change-to-speech time taken on 2026-10-23 on the build
  before the reading changes (its runs are kept under
  `target/perf/terminal-baseline-*`; the memory baseline is in
  [performance](performance.md), "Memory use"), in
  [performance](performance.md), for typing, a 20,000-line flood, a Pi
  stream at its recorded pace, a clock ticking once a second, and a
  full-screen program refreshing rows.
- [The terminals parity entry](parity/terminals.md), the crates'
  documents and these entries brought in line with the model.
- The whole terminal suite, `cargo xtask ci`, and the push.

Related items:

- Provisional, awaiting Dickson (2026-10-11): the console host's source
  raises no text change for a write that leaves the cursor where it was,
  as a full history scrolling whole lines may
  ([the model](design/terminal-model.md), "The terminals"); if a measurement
  on the installed build confirms it, follow NVDA, which has the same gap.
  A flood of identical lines cannot be counted whenever the history is
  full, a limitation in the model's "Limits".
- Windows Terminal and `FindText`: on 2026-10-24, in the pinned Windows
  Terminal 1.24.12741, a `FindText` that finds nothing returned a
  non-null range whose `GetText`, `CompareEndpoints` and
  `MoveEndpointByRange` fail with `E_FAIL` or `E_INVALIDARG` and whose
  other calls succeed, with no access violation; the console host behaves
  alike. The redesign does not use `FindText`; the current reading's
  anchor search goes when reading moves onto the core.
- An unsettled terminal read with nothing queued behind it is never
  retried; and `conhost_leave_flood` once never heard "blank" for an empty
  new console window, its first read giving nothing within 15 seconds.
- A typed character Windows Terminal does not echo in about 5% of runs:
  Windows Terminal sometimes raises no TextChanged event for it, only
  caret moves, and Verbatim reads the screen only on a text change (NVDA
  has the same dependency and would say the character late, with the next
  change). First settle whether Windows Terminal or UIA loses the event,
  with about 60 runs and a second, independent UIA client listening. The
  model reads on Windows Terminal's caret events as well as its text
  changes ([the model](design/terminal-model.md), "Reading rules", rule 1), which
  comes back with the outpost and Core step (Dickson, 2026-10-17: part of
  this redesign).
- The terminal scenario the lock-free listener's first step lets fail
  ([its design](design/listener-without-connections.md), section 10,
  step 1; Dickson, 2026-10-19): `windows_terminal_two_windows`, which in
  that step's runs failed 2 of
  10 because Windows Terminal raised focus events for its terminal
  control under two runtime ids, one rooted at the control's window
  (`[42, <window>, 4, n]`) and one of the XAML framework's own (`[14,
  ..., 4, n]`): the listener's same-element check took them for
  different elements, and "<title> terminal" was spoken two or three
  times where the scenario allows one ([UIA faults](uia-bugs.md),
  "Windows Terminal raises focus from a second runtime identity";
  evidence under `target/e2e-evidence/listener-no-connections/step1/scen/`).
- The console host's say-all repeats one row: say-all reads ahead by
  moving one line and then expanding to the line; the console host's text
  pattern moves by line through its whole buffer of 9,001 rows but
  clamps an expansion below the last row with text back onto that row,
  and nothing checks that the expanded line contains the point it was
  expanded from, so every later row comes back as the command's second
  row (`target/e2e-evidence/m42-key-holding-merge-2026-10-22/merged/measure_stale_caret_conhost/run1/say-all-then-right-0/flight-recorder.jsonl`).
  Proposed fix: take a unit that does not contain its point as the end
  of the text, one comparison inside the existing remote program, judged
  by a fake-source unit test, a mockapp provider whose expansion clamps,
  and a console host scenario step from a fresh NVDA capture. In the
  same redesign, the console's row count walks all 9,001 rows on each
  screen read that counts rows (`document_rows`,
  `crates/verbatim-uia-rops/src/terminal.rs` around 866), a possible
  cost to measure.
- `conhost_full_screen` races its program: a read taken before
  `altscreen.ps1` parks the cursor speaks "row N changed". It fails on
  the base build too.
- A lost read was seen to ask twice for the whole document, where one
  whole-document read should answer it.
- "hellook1" is heard in the recorded hand streams.
- Windows Terminal's launch focus sometimes speaks "... terminal" without
  "... window", and the desktop's announcement is sometimes cancelled
  (seen in the `*_up_typing`, `*_typing` and `*_commands` scenarios).
- `conhost_typing` missed a typed "o" in 2 of 6 runs on the base build.
- Pi's regular mode has no steps yet in the "Report status bar" and
  moving-by-command scenarios; they wait for Pi to be installed on the
  test machine.
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
- A review command's history read is placed with gaps in its checks:
  its answer carries no `from_start`, so a start the outpost refused
  during the read is placed as if counted from the read it names; an
  `Unanswered` history read is placed as no rows above the start, as if
  the document began there; and a whole-document history read is placed
  by the end of the memory's rows rather than by a start, so more than a
  screen of output no read has shown yet, printed during the round trip,
  shifts every placed row (reproduced on 2026-10-26: ReviewTop then
  ReviewNextLine read the wrong line); the outpost could count a whole
  read's rows from its newest start too, so Core places it by start.

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
- In the 28 verification runs of 2026-10-20, every passing run logged 1
  to 10 warnings "the device ran dry in the middle of an utterance: an
  audible gap" from `verbatim::audio` (debug build). They are not caused
  by the focus work and were not compared with older runs; they are to
  be looked at with the audio step.

### The focus pipeline rework

The scope, order of work and study are in
[the focus pipeline rework](design/focus-pipeline-rework.md). Agreed with Dickson on 2026-10-25 after a study of the pipeline found its
shape forced by three measured UIA facts (an element cannot leave the
process that received its event; a focus registration is desktop-wide;
a hung window named in a focus WinEvent holds every UIA event of a
process registered for focus), but about half of its ninety mechanisms
making Verbatim's processes agree on copies of one fact. Each item
removes one such reconciliation; the shape (the listener reads every UIA
focus, the outposts the rest, Core decides) stays.

- Repeats judged in Core. The listener keeps the last focus each
  application's reading thread read; a focus event on that element, by
  NVDA's `CompareElements` and a live `CurrentHasKeyboardFocus` on the
  kept element, never inferred from a cache, is not read again but sent
  as an ordinary focus fact with the kept facts and the event's cached
  states, and Core's duplicate rule drops it or takes it; the outpost
  does the same for a repeated MSAA focus. No protocol change. It
  removes `CoreFocus` and its recipients, the listener's held elements,
  the outpost's queue of unjudged focuses, and the follow-up on reads a
  watchdog abandons, and closes the hole where a focus returning to an
  element while Core's notice is in flight is never spoken. First count
  the repeats per scenario from the listener's and outposts' debug logs
  (`notepad_`, `windows_terminal_`, `explorer_folder_window`,
  `rapid_tabbing_in_settings`), and check that a name or value changed
  since the kept read reaches Core.
- The wait for a missing UIA focus moves into the listener, whose
  desktop-wide hook already receives every focus WinEvent beside the UIA
  focus on one clock; at its one-second bound it tells the outpost the
  focus is missing, and the outpost reads it in its ordered stream as
  now. It removes the drop evidence on the wire, the marker news, the
  100 ms look-back across two clocks and `CoreFocus` closing the wait.
  This replaces the placement of 2026-10-21. The read itself stays
  (decision 11; Dickson, 2026-10-25); once the rework lands, count how
  often it fires with NVDA closed and with it running, to test whether
  NVDA's presence causes UIA's undelivered focuses.
- The hand-over to a new outpost holds the outpost's own hook copies
  until the moment forwarding ended is known, instead of matching the
  listener's copies by counting, so each event arrives once by
  construction; first measure the relay's latency at start-up over fifty
  starts.
- Every observation stamped in microseconds from one clock, everywhere,
  so focus ordering is a total order.
- The marker that holds an application's later events until the
  listener's read arrives stays; build `text_box_first_character_after_tab`
  (`docs/design/focus-pipeline.md`, section 14), which measures what its
  removal would lose.
- Check, and fix if real, the possible defects the study found: the
  hand-over's maps never trimmed when forwarding's end never arrives; an
  ancestor released by `NodesHeld` spliced into a later report; the
  outbound merge leaving the outpost's states read out of step with
  Core; a focus-now answer's unjudged entry never drained.
- The window part of a focus's ancestry from window state Windows keeps,
  which never waits on the application's thread: the top-level window
  by `GetAncestor` with `GA_ROOT` of the nearest-window handle the
  listener already has, its title by `GetWindowText`, and its class name,
  so a slow application can delay only the inner elements its UIA parent
  walk adds, never the "<title> window" announcement; NVDA takes its
  foreground object from the operating system's foreground window in the
  same way (`api.setFocusObject`). Found on 2026-10-26: Windows
  Terminal raises its focus event before it has finished showing its
  window, and its one UI thread then blocks in
  `DwmExtendFrameIntoClientArea`; on the test VM's software-rendered
  compositor that took 0.3 to 1.5 seconds per window change, so every
  call into Terminal waited, and a parent step past the focus read's
  2-second budget left the ancestors unknown and Verbatim said only
  "<title> terminal". On a machine with a real graphics card the wait is
  milliseconds, so the reason is reliability, not that stall (Dickson,
  2026-10-26). Not a longer budget or retries, and nothing specific to
  Terminal: every call into a busy application stalls alike.

### The caret and selection rework

The scope, order of work and study are in
[the caret and selection rework](design/caret-selection-rework.md). Agreed with Dickson on 2026-10-25 after a study found the design's shape
right (a held key's caret read before it, the outpost watching for
evidence and comparing positions in the provider, Core choosing every
word) but a second layer, added before key holding closed the race it
answered, never removed: the caret and selection were kept in nine
places, and two known defects come from copies disagreeing (Control+A
followed by a repeated focus says nothing; a superseded key's answer
leaves Core's caret stale).

- One last read per text node in the outpost, replaced by every read;
  one request per key that reads the caret before it and opens the watch
  with that read as the key's baseline; every read reports facts; Core
  takes every report and every answer, late or superseded, as the newest
  caret by one stamp, the read's start; the baseline's life follows the
  focus Core holds.
- Removed: Core's history of eight caret reports and its Backspace and
  Delete prediction, the outpost's history of eight reads (now, with
  caret keys in an elevated window judged from the last read until
  uiAccess, M8.1), the time-zero record of Core's own caret move, the
  "since" check limited to a line, report numbers, the three modes of
  the selection baseline, the separate watch request, and the single
  superseded slot.
- Control+Z, Control+Y, Control+X, Control+V and Escape in a text field
  match NVDA: not held and silent, replacing the decision of 2026-10-08
  that spoke them; the `caret_keys` scenarios' expectations change.
- The judgement of a key's evidence stays in the outpost, named in D14
  as the one read-time decision outside Core, since it bounds reads on
  text changes.
- The outbound merge's clause against merging across a reply is checked
  and removed, its stated reason predating 505e3ad8.

### Deferred

- Review keys straight to the reducer
  ([communication](design/communication.md), section 11) is reconsidered
  when the command set is settled (Dickson, 2026-10-14).
- For a later milestone (Dickson, 2026-10-10): Verbatim echoes a typed
  character from its keyboard hook, before the application handles the
  key, where NVDA echoes from the application's typed-character event; a
  letter typed within about 40 ms of Tab is not echoed.
- The caret read made with a focus chooses remote or classic reading by
  the previous focus's window, since the outpost's window tracking is
  updated after the focus is published; only the read's cost differs.
- A question for Dickson: `notepad_editing` reads Notepad's spelling
  marks once before its second Control+Home, since Notepad raises no
  event when it marks a word, and fails when the idle-time spell check
  has not marked "gammaelta" yet; is there a sanctioned form of wait for
  a precondition with no event?

### Follow-ups from M4.1

M4.1 (stability) is complete and archived in [Completed
milestones](roadmap-done.md); these are its follow-ups still open or
deferred.

- The outpost crash inside UI Automation's saved-event delivery
  ([the phase 6 design](design/phase6-design.md), "Console windows as one
  application"): section 17 of the focus-pipeline design is built
  (152f09a), but `conhost_leave_flood`'s console host outpost failed
  fast again on 2026-10-12; its full dump, kept outside the repository,
  is to be read and section 17's fifty-run check made.
- Verbatim's own outpost once left a fact in its intake for 6.37 seconds
  (2026-10-11); it is to be reproduced with the intake and menu logs of
  f9d067d.
- An audit of every end-to-end scenario for timing races (2026-10-11),
  each race fixed by waiting for evidence, not a longer bound; and the
  agent's bring-forward, which always takes its slowest path (once 1.4
  seconds), its cause open. A known weakness of `measure_focus_explorer`'s
  key waits: each ends on its key's item, and "alpha 2 of 4" ends both
  Down 1 and Up 1, as "Inner 1 of 4" ends both Up 2 and Backspace, so a
  late repeat of one step's item arriving after the next key would end
  that key's wait; renaming cannot remove it, since the targets are
  `explorer_folder_window`'s.
- The classic .NET Framework Windows Forms combo box rule,
  element-selected events from the `ComboLBox` window reported as the
  combo box's value change (`NVDAObjects/UIA/__init__.py`, lines 2734 to
  2744 and 2779 to 2790), is deferred until Verbatim reads such a combo
  box in an application, since no .NET Framework application is in the
  suite (the audits' item 11, Dickson, 2026-10-20).
- The rule for a `Windows.UI.Core` combo box that has the `Value` pattern
  (lines 2795 to 2798) is not built, since every Settings combo box
  checked on 2026-10-20 lacks the pattern and takes the rule without it
  (`docs/parity/changes-and-selection.md`, "A combo box's value from its
  selected item").
- The events NVDA registers desktop-wide that Verbatim lacks (the audits'
  item 10) are deferred until each event is supported (Dickson,
  2026-10-20); `docs/parity/backends.md`, "UIA event registration",
  records the gap, and a foreground UIA progress bar that is not the
  focus is in M4.2, background progress bars in M4.3.
- What Verbatim does for a user whose first OneCore activation after
  logon takes longer than the synthesizer host's ten-second hang bound is
  open (Dickson, 2026-10-21).
- GitHub's CI on main failed on the two pushes before 2026-10-15 (runs
  38271366442 and 38544999189) in the ARM64 job's unit test
  `the_codeview_record_read_is_the_one_dbghelp_reports` in
  `verbatim-trace-waits` and in the hosted end-to-end job's
  `windows_terminal_two_windows` and `windows_terminal_leave_flood`;
  each is to be root-caused from the run logs
  (`gh run view <id> --log-failed`), since neither reproduces here.
- UIA's late or missing delivery of a new process's focus events, and the
  burst of gone senders (risks 2 and 13 of
  [the listener design](design/listener-without-connections.md)), are
  recovered by decision 11's step; the first `settings_system_page` or
  File Explorer run that meets the fault is to be read for the outpost's
  "the focus is read and reported" line and the speech after it.
- File Explorer can give the focus to its "Working on it..." placeholder
  and raise no later focus, UIA or `WinEvent`, so
  `measure_focus_explorer`'s step times out
  (`target/e2e-evidence/fixes-round-fifteen-2026-10-21/mfe-1-fail/`);
  decision 11's wait cannot recover it, and what File Explorer raises
  then, beyond a name change and three state changes on the items view,
  is to be found.
- Why the `delta` item inside Inner in run 39558690998 still carried the
  parent folder's window title, File Explorer's title lagging or a stale
  ancestor name, is to be checked.
- A console window read through MSAA is "pane" where NVDA gives its
  client object the legacy console overlay with the terminal role
  (`nvda/source/NVDAObjects/IAccessible/__init__.py`, lines 731 to 733,
  and `IAccessible/winConsole.py`; deferred, Dickson, 2026-10-21).
- On a fresh GitHub runner the first Verbatim start can say "Start toggle
  button not pressed" instead of "Desktop list", because the taskbar
  rehosts the Start button in a new PopupHost window and raises a UIA
  focus NVDA's checks would accept too (runs 39040893786 and
  38980790680); the choice between a throwaway warm-up start before the
  suite and an experiment to find what triggers the rehosting is
  deferred (Dickson, 2026-10-21).
- Notepad's "app not responding" sound played once in `notepad_editing`
  on 2026-10-18, about 2 seconds after a right arrow, and the rerun
  passed; it is investigated if it recurs.
- In File Explorer scenarios the listener's read once came 285
  milliseconds late, locally mostly the first nearest-window lookup
  waiting on a busy File Explorer (logged since f9fb32f7); the same is to
  be confirmed on GitHub's runner (deferred, Dickson, 2026-10-21).
- Windows Terminal sometimes raises its terminal's focus under a second
  runtime id, the one starting 14, so the terminal is said twice, or
  three times for a second window, which the tests accept as an optional
  utterance; the rule that recognizes it is deferred to Windows
  Terminal's app module in M5 (`target/e2e-evidence/full-suite-2026-10-17/`
  and `target/e2e-evidence/full-suite-2026-10-17-spares-off/`).
- One UIA focus event reached the listener 5 seconds late in a console
  host run on 2026-10-17; it is watched for again.
- The trace tool's symbol lookup for ARM64X images, such as ntdll, needs
  a follow-up that can be tested only on the ARM64 runner (deferred,
  Dickson, 2026-10-21).
- `line_key_flood` once read a half-written line on GitHub's runner; it
  is to be repeated there (deferred, Dickson, 2026-10-21).
- `text_box_selection_keys` fails at Control+Delete over the selection
  "alpha " in some runs (3 of 14 on fd1b92a5, 5 of 15 with the
  key-holding fixes), saying "alpha" where "beta", "selection removed"
  is expected: the key's watch answers from its first check, made before
  the edit control has handled the key, with the selection and the text
  unchanged. The likely cause, not yet confirmed: Core's word to compare
  for Control+Delete is "alpha" while the outpost reads the classic edit
  control's word at the caret as "alpha ", with the white space after
  it, so the unchanged text counts as evidence. The compared word and
  the outpost's word unit are to be made to agree
  (`target/e2e-evidence/m42-key-holding-fixes-2026-10-22/judges/`,
  `text_box_selection_keys-*`). The same mismatch for Delete, whose
  compared character left out a line break at the line's end, was fixed
  on 2026-10-23.
- Where NVDA's say-all starts is to be settled from NVDA's speech log
  before any say-all expectation changes: in every capture of
  2026-10-23 (`target/e2e-evidence/m42-text-captures-2026-10-23/`, items
  23a to 23d and the readme's note on say-all), NVDA's recorded speech
  began at the line after the caret's, in Notepad and in the text box,
  and either this NVDA build starts there or the capture add-on misses
  the first chunk of speech.
- Notepad's Find box differs from NVDA's capture of 2026-10-23 (item 21b
  in `target/e2e-evidence/m42-text-captures-2026-10-23/`): Control+F says
  Notepad's notification "Press F6 to go to new notification" first and
  "Find edit" and "selected beta" as two utterances, where NVDA says
  "Popup window", then "Find edit selected beta" as one, then the
  notification; this is the focus announcement's order with a
  notification, outside the caret keys, and is to be settled with a
  capture that does not depend on the text Notepad remembers in the box.
- The order of a selection change's parts: NVDA speaks every text
  selected, then every text unselected (`speakSelectionChange`,
  `nvda/source/speech/speech.py` 1367 to 1417), where Verbatim says the
  parts in the order of the text, the start side first. So when a
  selection is replaced by one apart from it, Verbatim says the old
  unselected first; and when a selection crosses its anchor, Verbatim
  says the start side first: on "alpha beta gamma", a line break, "delta
  epsilon" and a line break, Down Arrow, then
  Shift+Control+Home, then Shift+Control+End, where NVDA says "delta
  epsilon selected" then "alpha beta gamma unselected", Verbatim says
  the two the other way round. No capture has the replaced selection
  yet.
- Alt+Up and Alt+Down Arrow in the Windows Forms text box: NVDA, finding
  that the caret did not move after its wait, moves it by sentence
  itself and says the sentence, "delta epsilon"
  (`04d-textbox-docA-alt-down-run1.txt` in
  `target/e2e-evidence/m42-text-captures-2026-10-23/`), where Verbatim
  says nothing; whether Verbatim moves by sentence awaits Dickson's
  decision. The ledger's statement that these keys speak the line when
  the application moves the caret has no capture behind it.
- The remote caret program could leave out its two comparisons of the
  old and new selections' ends that judge whether they are apart when
  the old selection is known at build time to hold nothing
  (`previous_collapsed`, `crates/verbatim-uia-rops/src/caret.rs` around
  1519), since that case is a whole change already.
- Compare mode presses Verbatim+V (`kb:verbatim+v`) as Insert+V, which
  under NVDA types "v" into Notepad and corrupts the later steps of
  `notepad_editing`'s comparison.
- Mockapp's `a_focus_whose_walk_times_out_is_reported_with_its_containers_unknown`
  (`crates/mockapp/tests/remote_ops.rs`) failed once on 2026-10-23, in
  a full `cargo test -p mockapp` run on 4a82e004's tree, with mockapp
  not acknowledging a command within its timeout
  (`crates/mockapp/tests/common/mod.rs` 320); the next full run and five
  runs of it alone passed. Its cause is to be found if it recurs.
- Key holding's remaining gaps: no test covers a waiting key following
  the newest watch (`crates/verbatim-core/src/editing.rs` around 197 to
  201); no test or scenario covers a word read in a rich edit control;
  `Modifiers::after` keeps one flag for left and right Shift, Control
  and Alt together; a held key whose release is never seen leaves its
  entry in `held_downs`; and `call_counts.rs` pins the read before a
  held key by copying the production code's arguments rather than
  calling it.
- A double-click is checked only by hand (`dblclick2.ps1` in
  `target/e2e-evidence/m42-text-parity-2026-10-23/exploration/`): the
  agent has no mouse request, so no scenario pins "beta selected" for a
  double-click inside a word.
- The platform-neutral dependency check (`check_platform_neutral_deps` in
  `xtask/src/main.rs`) runs `cargo tree -e normal`, so a neutral crate's
  build-dependency on `windows-sys` or on a Windows-specific workspace
  crate is not caught; `winapi` is on neither banned list
  (`WINDOWS_BINDINGS` and `RAW_WINDOWS_BINDINGS`); and a neutral crate
  depending on a Windows-only workspace crate that itself uses no Windows
  bindings passes, since the check looks only for the bindings.
- Mockapp's `uia_combo_box_costs_exactly`
  (`crates/mockapp/tests/call_counts.rs`) depends on UI Automation keeping
  an element's state for roughly 500 milliseconds after
  `FindFirstBuildCache`: read warm, the combo box costs 6 provider calls
  fewer, so the exact count fails under desktop load. What the test pins
  awaits Dickson's decision.
- Mockapp's
  `a_remote_operation_in_progress_finishes_before_the_outpost_shuts_down`
  (`crates/mockapp/tests/shutdown.rs`) failed once in `cargo xtask ci`
  with the handler registrations [0, 0, 1, 1], and passed on reruns; its
  cause is to be found if it recurs.
- A console whose provider's formatting check gets no answer is probed
  and checked twice in one arbitration entry, through the `None` branch of
  `decide` in `crates/verbatim-outpost/src/arbitration.rs`; this predates
  a133815e, which keeps no verdict from a probe that timed out.
- The outpost's per-text-node selection baseline is not judged against
  the focus Core holds, as the listener's repeats are (decision D14,
  "Where the rules run"); whether it should be is a question for Dickson.
- A focus-now answer carries no application name and no Alt state, so an
  application rule such as the Alt+Tab switcher's cannot apply to a focus
  read on request.
- Table cells: the suppression of an unchanged row and column within the
  same table is not ported (pending Dickson), nor are header texts, spans,
  a cell's own coordinates text, or MSAA and IAccessible2 cells.
- On this Windows 11 build `TrayNotifyWnd`'s UIA subtree is empty, so the
  tray list (`crates/verbatim-outpost/src/outpost/shell_items.rs`) is
  empty unless the overflow flyout is open; and a double press of
  Verbatim+F11 opens the tray list rather than the taskbar list.
- A stuck flood console and its shell once outlived their scenario's
  cleanup, and the agent was force-killed to end them; the cleanup of such
  a console is not tested in normal runs.
- The listener's watchdog abandon path (`crates/verbatim-outpost/src/focus_read.rs`):
  a listener-read focus abandoned before it is sent is never judged by
  Core, so the listener drops no repeats in that application until a
  later focus is judged. It costs reads, not speech (pending Dickson).
- The focus ordering's tests: the hook-stamp test
  (`a_hooked_event_is_stamped_before_its_bookkeeping` in
  `crates/verbatim-outpost/src/outpost/mod.rs`) exercises only
  `take_in_hooked`, not that the hook callback calls it; mockapp's
  `a_focus_now_answer_is_stamped_to_the_microsecond` checks the
  milliseconds only to equal the microseconds divided by 1,000, which
  cannot tell one clock read from two; and `observed_no_earlier`, with
  stamps that mix milliseconds only and microseconds, is not a total order,
  so the outpost's running maximum of another application's focus
  (`crates/verbatim-outpost/src/outpost/held.rs`) can depend on arrival
  order.
- `notepad_editing`'s teardown fails when Notepad restores its earlier
  tabs, a setting of the test machine rather than of the scenario.
- `cargo xtask trace scenario` and `cargo xtask measure` look for the
  build under `<worktree>\target` and ignore a separate target
  directory (`CARGO_TARGET_DIR`), so a worktree building elsewhere
  measures a stale or missing build.

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
- The review cursor on objects that no longer exist (the section of that
  name, 2026-10-08): review commands read live from the application
  rather than from Core's cache; a Gone answer for the navigator's node
  re-seeds it from the focus, or says "No navigator object" when the dead
  node is the focus; the outpost reports held MSAA nodes it forgets with
  their window; and an end-to-end test in both terminals that a reviewed
  line stays on its text as new output is written below it ("Revised").
  Flat review's current-line command (`crates/verbatim-core/src/reduce.rs`,
  the flat `ReviewCurrentLine` arm) moves the review offset to the line's
  start, where NVDA's current-line command does not move the review
  position; it is fixed with this item ([review parity](parity/review.md)).

The work is split into three sessions (Dickson, 2026-10-21). The first
closed the items no longer listed here: the review cursor's bottom in a
standard edit control, state changes for an expired focus, the
compare-mode differences, and the smaller differences that session's
commits settled. The second takes toasts, the foreground UIA progress
bar, and the differences in a list the focus controls; the third takes
the review cursor on objects that no longer exist.

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

The design is [extensions](design/extensions.md); the order of work, with the
tests that prove each step, is [the M5 work plan](design/m5-extensions-work.md).
This entry keeps the scope and the exit criteria. M4.4 is the plan's
precondition, because the extension surface exposes the messages it moves.

- Modules as folders: a module is a `module.toml` beside a Wasm component;
  a built-in is a module folder under `extensions/builtin/`, never listed
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
- The guest workspace under `extensions/` with a permissively licensed SDK;
  every built-in written in Rust, Calculator and Notepad first.
- Every Windows inbox application as a built-in: Explorer (starting with the
  cosmetic fixes deferred from M3), Windows Terminal, Notepad, Calculator,
  Task Manager (proven by `mockapp` until M8.1 lets it be read end to
  end), Settings, Start and search, Quick Settings and Open With; sleep mode
  as a core per-application list rather than five modules.
- Windows Terminal's second terminal identity
  ([design](design/windows-terminal-twin-focus.md)) as the Terminal
  built-in's `on-event` hook, never core code: Windows Terminal sometimes
  raises its terminal's focus under a second runtime id starting 14 besides
  the usual one starting 42, and the listener announces each as a new
  focus. The hook answers `drop` for a 14 sender that is the twin of a
  current 42 focus and `adopt` for a 42 sender that is the twin of a
  current 14 focus, so the outpost keeps its node and follows the 42
  element. Twins share a process, the class `TermControl`, control type,
  the XAML framework, nearest window, and the 42 id's parts after
  `[42, window]` as the 14 id's last parts; a failed `CompareElements`
  means no twin. Its tests also check that the terminal is followed
  afterwards and cover two panes in one window. Before implementing,
  collect evidence that the runtime id's last part is never reused (pane
  splits, pane moves, closing and reopening panes, new tabs). Until then
  the tests accept one repeated "terminal" as an optional utterance, which
  does not cover the late repeat ("window, terminal, blank, terminal,
  terminal, blank", a 42 to 14 to 42 flip seen on GitHub's slower runner),
  so that can still fail scenarios. Evidence:
  `target/e2e-evidence/github-38988118089/`,
  `target/e2e-evidence/github-39032739474/`, and
  `target/e2e-evidence/wt-twin-identity/` (the judge's runs and logs,
  `identity_judge.py`, `suffix.py`, and the scratch scenario's patch).

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
  studying NVDA"). The compare check once symbol levels exist: at
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
  GitHub's elevated runner, run 38031529579). Capture what NVDA says for
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
