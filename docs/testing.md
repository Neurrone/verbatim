# The testing standard

Every test in Verbatim meets this standard: unit tests, `mockapp`'s cross-process tests, and the end-to-end scenarios. A test that cannot meet it is redesigned, not weakened. Code review checks every test change against it.

The standard was set on 2026-10-07, after an audit found tests that passed without testing what they claimed: speech checks that skipped unexpected utterances, scenarios that accepted whichever of two applications a machine had, and tolerances that let real regressions through.

The measure set, the end-to-end registry's `measure` group (`measure_` scenarios), is measurement, not tests, and is outside this standard's assertion rules. It is the workload `docs/design/communication.md` section 11 measures Verbatim's latency with, and it is excluded from the suite: no suite run includes it, and every suite command deselects it with `--skip measure_`. Its scenarios assert the startup speech and the desktop's announcement, then collect every utterance without asserting it, and pass by completing. They keep the rules that make a measurement sound: the same starting desktop, one fixed target each, waits only for evidence, and windows brought forward as a taskbar button does. A measure scenario passing is never evidence that Verbatim behaves correctly.

Beside the measure set, two cost benchmarks are measurement, not tests, under the same terms: `crates/verbatim-text/tests/terminal_cost.rs` (the terminal memory and reading rules) and `crates/verbatim-core/tests/terminal_cost.rs` (the reducer's cost of a terminal's reads). Each is ignored, so no test run includes it, is run in a release build with `--ignored --nocapture`, and asserts nothing about the times (Dickson, 2026-10-26).

Compare mode (`docs/tooling/nvda.md`, "Comparing a scenario"), which runs a scenario against NVDA and reports what it expects beside what NVDA said, is a research instrument, not a test: it asserts nothing and never runs in CI, so this standard is unchanged by it.

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
- A third exception, decided by the owner (2026-11-03): the step of `verbatim_lifecycle` that exits Verbatim through its menu. Whether the desktop's focus is spoken before the exit cancels speech depends on when the exit overtakes the focus event, as in NVDA, so only the end state is asserted: the exit sound is the last utterance before the speech connection ends, and Verbatim exits with code 0. The utterances between the Enter and the exit sound are taken unasserted. Verbatim has exited, so the run dumps neither Core's focus nor the flight recorder after that body; its logs and timeline are kept.
- A mismatch is reported with the expected and actual sequences, escaped, and the first utterance and character that differ.
- Counts are exact numbers, not bounds. State is compared whole, not as a subset.
- A test's expectation is fixed before it runs. It never reads Verbatim's answer to decide what to expect next; when it needs the system's state, such as a lock key's, it reads that state independently first.
- Reducer tests assert every effect an input produces, not only the first.

### Optional utterances

Where an application's own timing decides whether some speech comes at all, or how many times, a scenario may declare an optional utterance at a fixed place in its expected sequence (decided by the owner, 2026-10-17). In that place the assertion accepts the utterance's exact text heard in full, cut off (by later speech or anything else that cancels it), or not said at all, and, where a repeat is declared, the same up to the declared number of times in a row. Nothing else may appear there: another text fails the assertion as any mismatch does, and there is no substring matching. Every other utterance in the sequence is asserted exactly. An optional utterance stands at one place, and may be accepted at either of two places only where the owner has recorded that exception in the list below. It is followed in its step by an utterance always said, whose arrival shows it was not said. Where a step begins with optional utterances, they may also match speech that arrived before the step's input was sent, which the check before the input then leaves for the step instead of failing; speech arriving after the input matches them in sequence, and any other utterance waiting before the input fails the check as unasserted speech always does.

An optional utterance is only for speech whose presence or repetition the application's non-deterministic behaviour causes, never for a timing of Verbatim's own. Each use is approved by the owner and listed below with the reason. The run reports what became of each one: absent, heard in full, or cut off, and how many times when it was repeated. It is printed after the scenario, noted on its timeline, and written to its run summary, so a change in how often it comes stays visible.

The approved uses:

- File Explorer's "Working on it..." placeholder, on the folder window's opening and after Enter and after Backspace in `explorer_folder_window`, before the item's focus speech: as the folder loads, File Explorer gives the placeholder the keyboard focus for a moment or not, depending on how quickly the machine loads the folder, and then gives it to the item, which stays required. It is the one exception accepted at either of two places, before "Items View list" (before the window's title on the opening) or between it and the item, and at most once in all (Dickson, 2026-10-21), because which place it takes depends on whether File Explorer removes the placeholder after the focus listener's nearest-window walk, so that the listener's classic read of it fails, or its read budget runs out, and it is reported with its ancestors unknown, as NVDA's own handling depends on when the element goes (`docs/parity/focus-events.md`, "A focus whose element is gone before its read").
- A Windows Terminal window's "<title> terminal", repeated before the one heard in full, as the window opens in every Windows Terminal scenario, up to once for a window opened on its own: in some launches Windows Terminal raises the terminal's focus under a second runtime id besides the usual one, so the terminal is announced again. Its repeat may be followed by its line, "blank", before the one heard in full, and only after a repeat (Dickson, 2026-10-26), because Windows Terminal raises the two focuses about 40 ms apart, so Verbatim may finish the first announcement, line included, before the second cancels it; the rule for the duplicate focus comes with Windows Terminal's app module in M5.
- Settings' "sound: suggestions-closed" a second time after Escape closes the search box's suggestions, in `settings_search_suggestions_reentered`, at the start of the Tab step that follows, before the account button, and accepted before the Tab is sent: Settings sometimes raises the search box's ControllerFor change twice, about a millisecond apart, NVDA receives the second in some presses and plays the sound for each it receives (`docs/parity/changes-and-selection.md`, "Suggestions appearing and going away"), and the second sound may land before or after the Tab is sent (Dickson, 2026-10-26).
- A second window opened in the running Windows Terminal, in `windows_terminal_two_windows` and `windows_terminal_leave_flood`: its "<title> terminal" repeated, before the one heard in full alone, or after it, until the first prompt, with or without its line, "blank", and up to twice in all. Windows Terminal raises the new terminal's focus under more than one runtime id, each repeat reads the caret's line on its own, and a repeat whose element the outpost cannot find has no line; NVDA announces each repeat too, with the line only where it could read the text (`docs/parity/terminals.md`, "Windows and tabs"). This is the second exception accepted at either of two places (Dickson, 2026-10-26); a "blank" after the one heard in full is accepted only after a repeat of the terminal.

## Waits are for evidence

- No fixed sleeps, polling loops, retries, or "wait until quiet" in place of evidence. Every wait names the event or reply it waits for. `cargo xtask ci` fails on a fixed sleep or a retry counter in test code.
- Evidence that Verbatim has handled an input comes from Verbatim: the harness numbers every key it injects, and Verbatim reports each number once everything the key caused is queued. Evidence about an application comes from the specific event Core receives, never from a period of silence.
- Every wait in the agent or the control plane ends before the client's read timeout, so a failure is a named timeout on a live connection, not a broken one.
- A test does not time, or bound tightly, anything that goes through a read the Desktop Window Manager answers, such as a window's title-bar information, which the MSAA proxy reads: it can fall seconds behind whenever windows are made or destroyed anywhere in the session (`docs/tooling/troubleshooting.md`).
- Response times are asserted. Each step has a budget from event to queued speech and from event to audio, set from measurements; a long timeout only bounds a hang and never stands in for a budget. Budgets are enforced by default in local runs, and on GitHub's runners an explicit setting turns enforcement off, so the numbers there are recorded but not enforced (Dickson, 2026-10-08, reaffirmed 2026-10-09). This is decided but not yet built: today the suite records the latency of each utterance and asserts none (`SpeechCollector::latency_rows`, `crates/verbatim-e2e/src/speech.rs`).
- No tolerances that let a regression pass. Where a measurement varies by machine, such as the terminal flood's slowdown, it is recorded for trends rather than asserted with a loose limit.

## Failures are never hidden

A test known to fail is listed with its cause and fix in [the known failures](flaky-tests.md); it is not skipped or loosened to pass.

- A skipped test is reported as skipped, never as passed.
- Cleanup closes what the test opened, by process ID or by its window, and fails the test when it cannot. When the test started the program, the process that owned the window must exit. Nothing is killed by image name.
- Any of Verbatim's own processes that exits unexpectedly during a scenario fails it, and the crash dump, when crash dumps are configured, is kept with the scenario's artifacts.
- Errors are never swallowed. Failure artifacts (timeline, logs, flight recorder, Core's focus and navigator) are always kept, and losing one is itself a failure. A recording that cannot start or finish is a failure too.
- No test is `#[ignore]`d without a recorded decision, and an ignored test's behaviour is covered elsewhere.
- Everything a demonstration shows is covered by a test.

## Tests exercise the real code

- A test measures or exercises the production code, never a copy of its logic written in the test.
- A failure path is tested by making the real operation fail at a fault point placed just before it (`verbatim-faults`, [its guide](crates/verbatim-faults.md)), so the code takes the path the real failure takes; the test selects the one call that fails, and the point must fire exactly as often as armed. Fault points are compiled only into tests: a crate's own unit tests reach its points, and another crate's fault tests are test targets with `required-features = ["faults"]`, or tests compiled only under the feature, built only by `cargo xtask faults`, which `cargo xtask ci` runs as its fault step. An observation point is the same mechanism where nothing fails: a test waits for it as evidence that the code reached a wait before it checks that the wait holds, or asserts exactly how often, or for which reason, the code got there, such as one registration call ended by UIA's connection timeout or a caret watch ended by its poll, in place of a bound on the elapsed time.
- What a test's documentation says it checks is exactly what it asserts.

## The harness does not change what it measures

- The harness injects only the input the scenario is about. It never taps keys, or uses Alt+Tab, to bring a window forward; a window that refuses the foreground fails the test. Even where switching windows is what a scenario is about, as in the two-window scenarios, the harness brings the window it opened, found by its title marker, to the front, as clicking its taskbar button does and as NVDA's system tests do with `SetForegroundWindow` (`Scenario::bring_window_forward`; Dickson, 2026-10-10). Alt+Tab goes to the most recently used window, which the harness cannot predict, so it could reach a window the scenario did not open.
- Every scenario starts from the same state: all windows minimized to the desktop, whether the run is recorded or not. Recording and other instrumentation change nothing else about the desktop or the load a run sees.

## The crates on a reducer's path

A replay or a generated case must depend only on its inputs, and every project that built a simulation reports leaks of exactly this kind. So the crates on a reducer's path allow no hash map or set, no clock, no randomness, no thread, no environment, file, process or network call, no minted trace id, no call during a transition and no floating-point function from the platform's library. The tier check (`xtask/src/tiers.rs`) derives the set from the dependency graph, with `verbatim-input` listed beside the reducers. `verbatim-speech` stays outside the rule (Dickson, 2026-10-31): it runs after the reducer and owns the synthesizer threads, and its maps are only looked up.

How the rule is enforced:

- Each covered crate's `clippy.toml`, identical to `verbatim-core`'s, disallows the types and methods.
- The ownership scan (`xtask/src/ownership.rs`) keeps those files equal, forbids silencing them outside its listed exceptions, forbids the whole modules that Clippy names function by function, and forbids `tracing`'s `enabled!` family and `proptest`'s macro and default configuration.
- The tier check holds each covered crate's direct third-party dependencies to an allow-list.

What the rule means in code:

- `BTreeMap` and `BTreeSet` everywhere on the path (Dickson, 2026-10-31), and a fixed-hasher map only where a measurement shows a cost that matters, named with its figure; none is taken. Reason: one kind of map leaves no key classification for review, so Clippy can enforce the rule whole. The terminal alignment's five lookups by row hash are ordered collections too, accepting 12 to 62% more time on those lookups' cases than fixed-hasher maps because the simpler code is preferred (Dickson, 2026-11-01; `docs/performance.md`).
- Times and trace ids are inputs: microseconds on the run's shared clock, never an `Instant`, and a trace id carried by the input that caused the work, `TraceId::from_raw` in tests. Reason: an `Instant` cannot be recorded or generated, and the mint is a process-wide counter.
- A reducer is pure with calls as its effects (Dickson, 2026-10-31): a local read it needs is an ask, an effect answered by an input.
- Each generated case runs twice from a new owner and the two effect sequences are compared by their `Debug` text, which is ordered because every map on the path is ordered.

A third-party crate the covered crates call gets the same check when it is added to the allow-list and when a version change is reviewed (Dickson, 2026-10-31): the functions called from the path, and the dependencies they reach, are read from source for a randomly seeded hash map whose order reaches a result, a clock, randomness, a thread, the environment or a file, and global mutable state that changes a result. Verdicts, at the versions in the lock file on 2026-11-01:

- `jieba-rs` 0.11.0: deterministic. Its dictionary is embedded, its maps are `rustc-hash`'s fixed-seed maps, its trie's hash is fixed, and its thread-local scratch buffers are cleared for each call.
- `icu_segmenter`, `icu_normalizer`, `icu_properties` and `icu_locale_core` 2.2.0: deterministic. Their data is compiled in, and the word segmenter's LSTM computes with `core_maths` over the pure Rust `libm`, so x64 and ARM64 agree.
- `unicode-width` 0.2.2: deterministic, static tables.
- `serde` 1.0.228: deterministic; its hash map impls are reached only by hash maps, which the covered crates have none of.
- `serde_json` 1.0.150: deterministic while its `preserve_order` feature is off, which nothing enables.
- `vte` 0.15.0: deterministic while its `ansi` feature, the one with a clock read, is off, which nothing enables.
- `tracing` 0.1.44: its dispatcher is global state that decides only whether an event is written, so the scan forbids asking it whether one is enabled.
- `proptest` 1.12.0: deterministic as called, from `random_config()`, which fixes the seed and turns off failure files, forks and timeouts; its default configuration reads the environment, so the scan forbids it.

Limits: the verdicts on `serde_json` and `vte` rest on features nothing enables, which `workspace-hack` could unify in, though neither would make a result depend on anything but its input; a `tracing` field with a side effect would make the logic depend on the subscriber, which is a review rule; and the idle barrier and the GUI's lifecycle stay outside the covered crates.

### Exploration tests

Each generated test in the gate has a fixed seed and case count and reads nothing from the environment. Beside it, an `#[ignore]`d exploration test calls the same machine with a seed and case count read from `VERBATIM_EXPLORE_SEED` and `VERBATIM_EXPLORE_CASES`, and fails if either is missing. Its file is one of the ownership scan's exceptions for `std::env` (an `expect` of `clippy::disallowed_methods` with its reason). This is the recorded decision for those ignored tests: only the scheduled workflow (`.github/workflows/explore.yml`) runs them, and a failure it finds is shrunk and added to the owner's fixed cases as an explicit action list, never kept as a seed. Reason: the environment is read only by a test the gate never runs, in a file the scan names.

A failing generated test, exploration or enumeration prints its smallest failing case as a line ready to paste into its test's list of fixed cases (`fixed()` where the cases kept are failures, otherwise `found()`), in that test's own notation, with the command that runs that list and the command that runs the failing test again, which for an exploration carries its seed and case count. A proptest case is the one its runner shrank to; an enumeration walks each depth in turn, or keeps every failure, so the case printed is one of the shortest. Reason: a failure becomes a fixed case by pasting one line, with no hand translation from a `Debug` dump.
