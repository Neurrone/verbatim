# The testing standard

Every test in Verbatim meets this standard: unit tests, `mockapp`'s cross-process tests, and the end-to-end scenarios. A test that cannot meet it is redesigned, not weakened. Code review checks every test change against it.

The standard was set on 2026-10-07, after an audit found tests that passed without testing what they claimed: speech checks that skipped unexpected utterances, scenarios that accepted whichever of two applications a machine had, and tolerances that let real regressions through.

The measure set, the end-to-end registry's `measure` group (`measure_` scenarios), is measurement, not tests, and is outside this standard's assertion rules. It is the workload `docs/design/communication.md` section 11 measures Verbatim's latency with, and it is excluded from the suite: no suite run includes it, and every suite command deselects it with `--skip measure_`. Its scenarios assert the startup speech and the desktop's announcement, then collect every utterance without asserting it, and pass by completing. They keep the rules that make a measurement sound: the same starting desktop, one fixed target each, waits only for evidence, and windows brought forward as a taskbar button does. A measure scenario passing is never evidence that Verbatim behaves correctly.

Compare mode (`docs/nvda-transcript.md`, "Comparing a scenario"), which runs a scenario against NVDA and reports what it expects beside what NVDA said, is a research instrument, not a test: it asserts nothing and never runs in CI, so this standard is unchanged by it.

## One target, one code path

A test runs against one fixed application and exercises one backend. Nothing about what it runs against, or what it expects, depends on the machine.

- Win32 and MSAA edit controls are tested in a Windows Forms text box the harness creates, which behaves the same everywhere.
- UIA documents are tested in Windows 11 Notepad. Those scenarios are local-only (`VERBATIM_E2E_SKIP_LOCAL_ONLY`), since GitHub's Windows Server runner has classic Notepad instead.
- Editing behaviour is covered against both: each editing scenario exists once for the Windows Forms box and once for Windows 11 Notepad. Formatting, such as spelling errors, has tests of its own.
- Terminal behaviour is covered in both terminals: each terminal scenario exists once for Windows Terminal and once for the console host, and asserts the program that owns the window.
- Scripted providers are tested in `mockapp`.

A test never falls back to another target when its own is missing, and never chooses its expectation from what the application answered. When a target is missing, the test fails.

### One target, one scenario

Where a scenario exists for two targets, each target's scenario is its own code: one straight sequence of steps with its own exact expectations written inline, such as "line feed" in the text box's editing scenario and "carriage return" in Notepad's. Shared helpers are for mechanics only: opening a target, saving, pressing a key and asserting the utterances the caller gives. Nothing chooses a step or an expectation by target, so no enum or flag naming the target decides what is pressed or what is heard, and reading one scenario's code tells exactly what it does and expects.

## Exact assertions

- Speech is asserted as an exact sequence: every utterance in order, with its exact text, each heard in full, with nothing else in between and nothing after the last one. There is no substring matching, no "any of these", and no skipping of utterances that do not match. The one allowance is an optional utterance, below, approved use by use.
- Every utterance counts, an empty one and a sound played on its own included, and so does how it ended: heard in full, unless the step interrupts it, when the assertion says it was cut off. A step that interrupts speech first asserts that the utterance it interrupts has started to play.
- Speech is never discarded unchecked. A scenario ends by asserting that nothing further was said: once Verbatim says it has handled the scenario's last input and is idle, nothing may have been queued that no assertion matched.
- A scenario never moves on past speech it has not asserted. Injecting input while an utterance no assertion has matched is waiting is a harness error.
- Startup speech is asserted at the start of every scenario, like any other speech.
- One exception, decided by the owner: `rapid_tabbing_in_settings`, whose intent is the end state after a burst of focus changes. Which controls the burst passes through get announced depends on timing, as in NVDA, so that speech is asserted only to have been cut off, never heard in full; its text is not asserted. The burst ends on a different control from the one it started on, so its final announcement always comes; that announcement, everything after it, and the navigator are asserted exactly.
- A second exception, decided by the owner (2026-10-15): the `*_control_flood` scenarios. Which bursts Verbatim reads a terminal flood in depends on timing, so the flood's speech before Control is asserted only by its first line, which must start to play, and its last line, which must be queued before Control is pressed; every utterance in between is taken unread. Its endings are asserted: in order, any heard in full before Control, then the rest cut off, the last line always among them. Everything after Control is asserted exactly.
- A mismatch is reported with the expected and actual sequences, escaped, and the first utterance and character that differ.
- Counts are exact numbers, not bounds. State is compared whole, not as a subset.
- A test's expectation is fixed before it runs. It never reads Verbatim's answer to decide what to expect next; when it needs the system's state, such as a lock key's, it reads that state independently first.
- Reducer tests assert every effect an input produces, not only the first.

### Optional utterances

Where an application's own timing decides whether some speech comes at all, or how many times, a scenario may declare an optional utterance at a fixed place in its expected sequence (decided by the owner, 2026-10-17). In that place the assertion accepts the utterance's exact text heard in full, cut off (by later speech or anything else that cancels it), or not said at all, and, where a repeat is declared, the same up to the declared number of times in a row. Nothing else may appear there: another text fails the assertion as any mismatch does, and there is no substring matching. Every other utterance in the sequence is asserted exactly. An optional utterance stands at one place, and may be accepted at either of two places only where the owner has recorded that exception in the list below.

An optional utterance is only for speech whose presence or repetition the application's non-deterministic behaviour causes, never for a timing of Verbatim's own. Each use is approved by the owner and listed below with the reason. The run reports what became of each one: absent, heard in full, or cut off, and how many times when it was repeated. It is printed after the scenario, noted on its timeline, and written to its run summary, so a change in how often it comes stays visible.

The approved uses:

- File Explorer's "Working on it..." placeholder, on the folder window's opening and after Enter and after Backspace in `explorer_folder_window`, before the item's focus speech: as the folder loads, File Explorer gives the placeholder the keyboard focus for a moment or not, depending on how quickly the machine loads the folder, and then gives it to the item, which stays required. It is the one exception accepted at either of two places, before "Items View list" (before the window's title on the opening) or between it and the item, and at most once in all (Dickson, 2026-10-21), because which place it takes depends on whether File Explorer removes the placeholder after the focus listener's nearest-window walk, so that the listener's classic read of it fails, or its read budget runs out, and it is reported with its ancestors unknown, as NVDA's own handling depends on when the element goes (`docs/parity/focus-events.md`, "A focus whose element is gone before its read").
- A Windows Terminal window's "<title> terminal", repeated before the one heard in full, as the window opens in every Windows Terminal scenario, up to once for a window opened on its own: in some launches Windows Terminal raises the terminal's focus under a second runtime id besides the usual one, so the terminal is announced again.
- Settings' "sound: suggestions-closed" a second time after Escape closes the search box's suggestions, in `settings_search_suggestions_reentered`, before the Tab's speech: Settings sometimes raises the search box's ControllerFor change twice, about 5 ms apart, and the sound is played for each, as NVDA plays it for each it receives (`docs/parity/changes-and-selection.md`, "Suggestions appearing and going away"; Fable, 2026-10-22, pending Dickson's acceptance).
- A second window opened in the running Windows Terminal, in `windows_terminal_two_windows` and `windows_terminal_leave_flood`: its "<title> terminal" repeated, before the one heard in full alone, or after it, until the first prompt, with or without its line, "blank", and up to twice in all. Windows Terminal raises the new terminal's focus under more than one runtime id, each repeat reads the caret's line on its own, and a repeat whose element the outpost cannot find has no line; NVDA announces each repeat too, with the line only where it could read the text (`docs/parity/terminals.md`, "Windows and tabs"). This is the second exception accepted at either of two places (Fable, 2026-10-22, pending Dickson's acceptance); a "blank" after the one heard in full is accepted only after a repeat of the terminal.

## Waits are for evidence

- No fixed sleeps, polling loops, retries, or "wait until quiet" in place of evidence. Every wait names the event or reply it waits for. `cargo xtask ci` fails on a fixed sleep or a retry counter in test code.
- Evidence that Verbatim has handled an input comes from Verbatim: the harness numbers every key it injects, and Verbatim reports each number once everything the key caused is queued. Evidence about an application comes from the specific event Core receives, never from a period of silence.
- Every wait in the agent or the control plane ends before the client's read timeout, so a failure is a named timeout on a live connection, not a broken one.
- A test does not time, or bound tightly, anything that goes through a read the Desktop Window Manager answers, such as a window's title-bar information, which the MSAA proxy reads: it can fall seconds behind whenever windows are made or destroyed anywhere in the session (`docs/tooling/troubleshooting.md`).
- Response times are asserted. Each step has a budget from event to queued speech and from event to audio, set from measurements; a long timeout only bounds a hang and never stands in for a budget. Budgets are enforced by default in local runs, and on GitHub's runners an explicit setting turns enforcement off, so the numbers there are recorded but not enforced (Dickson, 2026-10-08, reaffirmed 2026-10-09). This is decided but not yet built: today the suite records the latency of each utterance and asserts none (`SpeechCollector::latency_rows`, `crates/verbatim-e2e/src/speech.rs`).
- No tolerances that let a regression pass. Where a measurement varies by machine, such as the terminal flood's slowdown, it is recorded for trends rather than asserted with a loose limit.

## Failures are never hidden

- A skipped test is reported as skipped, never as passed.
- Cleanup closes what the test opened, by process ID or by its window, and fails the test when it cannot. When the test started the program, the process that owned the window must exit. Nothing is killed by image name.
- Any of Verbatim's own processes that exits unexpectedly during a scenario fails it, and the crash dump, when crash dumps are configured, is kept with the scenario's artifacts.
- Errors are never swallowed. Failure artifacts (timeline, logs, flight recorder, Core's focus and navigator) are always kept, and losing one is itself a failure. A recording that cannot start or finish is a failure too.
- No test is `#[ignore]`d without a recorded decision, and an ignored test's behaviour is covered elsewhere.
- Everything a demonstration shows is covered by a test.

## Tests exercise the real code

- A test measures or exercises the production code, never a copy of its logic written in the test.
- What a test's documentation says it checks is exactly what it asserts.

## The harness does not change what it measures

- The harness injects only the input the scenario is about. It never taps keys, or uses Alt+Tab, to bring a window forward; a window that refuses the foreground fails the test. Even where switching windows is what a scenario is about, as in the two-window scenarios, the harness brings the window it opened, found by its title marker, to the front, as clicking its taskbar button does and as NVDA's system tests do with `SetForegroundWindow` (`Scenario::bring_window_forward`; Dickson, 2026-10-10). Alt+Tab goes to the most recently used window, which the harness cannot predict, so it could reach a window the scenario did not open.
- Every scenario starts from the same state: all windows minimized to the desktop, whether the run is recorded or not. Recording and other instrumentation change nothing else about the desktop or the load a run sees.
