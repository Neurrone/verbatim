# The testing standard

Every test in Verbatim meets this standard: unit tests, `mockapp`'s cross-process tests, and the end-to-end scenarios. A test that cannot meet it is redesigned, not weakened. Code review checks every test change against it.

The standard was set on 2026-10-07, after an audit found tests that passed without testing what they claimed: speech checks that skipped unexpected utterances, scenarios that accepted whichever of two applications a machine had, and tolerances that let real regressions through.

## One target, one code path

A test runs against one fixed application and exercises one backend. Nothing about what it runs against, or what it expects, depends on the machine.

- Win32 and MSAA edit controls are tested in a Windows Forms text box the harness creates, which behaves the same everywhere.
- UIA documents are tested in Windows 11 Notepad. Those scenarios are local-only (`VERBATIM_E2E_SKIP_LOCAL_ONLY`), since GitHub's Windows Server runner has classic Notepad instead.
- Editing behaviour is covered against both: each editing scenario exists once for the Windows Forms box and once for Windows 11 Notepad. Formatting, such as spelling errors, has tests of its own.
- Terminal behaviour is covered in both terminals: each terminal scenario exists once for Windows Terminal and once for the console host, and asserts the program that owns the window.
- Scripted providers are tested in `mockapp`.

A test never falls back to another target when its own is missing, and never chooses its expectation from what the application answered. When a target is missing, the test fails.

## Exact assertions

- Speech is asserted as an exact sequence: every utterance in order, with its exact text, each heard in full, with nothing else in between and nothing after the last one. There is no substring matching, no "any of these", and no skipping of utterances that do not match.
- Speech is never discarded unchecked. A scenario ends by asserting that nothing further was said.
- Counts are exact numbers, not bounds. State is compared whole, not as a subset.
- A test's expectation is fixed before it runs. It never reads Verbatim's answer to decide what to expect next; when it needs the system's state, such as a lock key's, it reads that state independently first.
- Reducer tests assert every effect an input produces, not only the first.

## Waits are for evidence

- No fixed sleeps, polling loops, retries, or "wait until quiet" in place of evidence. Every wait names the event or reply it waits for.
- Response times are asserted. Each step has a budget from event to queued speech and from event to audio, set from measurements; a long timeout only bounds a hang and never stands in for a budget.
- No tolerances that let a regression pass. Where a measurement varies by machine, such as the terminal flood's slowdown, it is recorded for trends rather than asserted with a loose limit.

## Failures are never hidden

- A skipped test is reported as skipped, never as passed.
- Cleanup closes what the test opened, by process ID, and fails the test when it cannot. Nothing is killed by image name.
- Errors are never swallowed. Failure artifacts (timeline, logs, flight recorder) are always kept, and losing one is itself a failure.
- No test is `#[ignore]`d without a recorded decision, and an ignored test's behaviour is covered elsewhere.
- Everything a demonstration shows is covered by a test.

## Tests exercise the real code

- A test measures or exercises the production code, never a copy of its logic written in the test.
- What a test's documentation says it checks is exactly what it asserts.

## The harness does not change what it measures

- The harness injects only the input the scenario is about. It never taps keys, or uses Alt+Tab, to bring a window forward; a window that refuses the foreground fails the test.
- Every scenario starts from the same state: all windows minimized to the desktop, whether the run is recorded or not. Recording and other instrumentation change nothing else about the desktop or the load a run sees.
