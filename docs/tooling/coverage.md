# Code coverage

How to measure which lines of Verbatim each level of tests runs: on this
machine with `cargo xtask coverage`, and on GitHub with a coverage run of
the CI workflow, which adds the end-to-end suite.

Coverage is line coverage from LLVM's source-based instrumentation, read
with cargo-llvm-cov, per test level:

- unit: every test of the workspace but mockapp's, run by cargo-nextest
  with the filters and one-at-a-time runs of `cargo xtask ci`
  (`docs/tooling/setup.md`, "What cargo xtask ci runs").
- mockapp: mockapp's tests (`docs/crates/mockapp.md`), run the same way.
- faults: the fault tests as `cargo xtask faults` runs them, the
  `FAULT_TESTS` of `xtask/src/main.rs`, each target through `cargo test`
  with its package's `faults` feature.
- e2e: the end-to-end suite as CI runs it, in groups, against an
  instrumented build; GitHub only.
- merged: all of them together.

The fault tests are a level of their own, measured last, rather than
part of the unit and mockapp levels, because their build replaces
binaries whose file names do not change with a feature, such as
`mockapp.exe`: a report made after it finds no binary for the profiles
the ordinary build's processes wrote, and in one report with mockapp's
tests it lost 1061 of the mockapp level's covered lines. The lines only
the `faults` build compiles, the fault points and the hooks they call,
can be reached by this level alone.

The doctests are not measured, since instrumenting them needs a nightly
compiler.

## Running it on this machine

Install the pinned cargo-llvm-cov once; `cargo xtask coverage` refuses
any other release, and GitHub Actions installs the same one, so a new
release changes nothing until the pin, `LLVM_COV_VERSION` in
`xtask/src/coverage.rs`, is moved in both places:

```
cargo install cargo-llvm-cov --version 0.9.1 --locked
```

Then:

```
cargo xtask coverage
```

It adds the toolchain's `llvm-tools-preview` component when it is
missing, removes the workspace's instrumented artifacts and every old
profile and report, then runs each level's tests, instrumented, and
reports the level on its own: an HTML report at
`target/coverage/<level>/html/index.html` and an lcov file at
`target/coverage/<level>.lcov`, whose paths are relative to the
workspace. It ends by printing each level's line coverage, such as
`unit: 61.2% of lines (612 of 1000)`. A failing test stops nothing:
every level's tests run and are reported, from the profiles they wrote,
and the command then fails, naming each step that failed.

The instrumented build goes to `target/llvm-cov-target`, apart from
`target/debug`, and took 5.2 GB; `cargo llvm-cov clean` empties it. On
the 12-processor development machine the whole command took 5.5 minutes
with no build cached, of which the tests took 84 seconds, 1.08 times
their uninstrumented time. Like `cargo xtask ci`, the run makes
windows on the desktop it was started from, so it is not run while a
scenario is.

On Linux it measures the unit level alone, the crates in `LINUX_CRATES`
(`xtask/src/main.rs`) through `cargo test`, as `cargo xtask ci` tests
them there.

## Processes that end without exiting

The profile runtime writes a process's counts when it exits, but the
outposts and the focus listener end themselves with `TerminateProcess`
(`docs/uia-bugs.md`, "A process that used UIA can hang or crash as it
exits"), and so does mockapp's test runner. Each of them writes its
profile first, through `verbatim_process::child::write_coverage_profile`,
which does that in a build instrumented for coverage (`cfg(coverage)`,
which cargo-llvm-cov sets) and nothing in any other.

The synthesizer host exits on its own when its command pipe from
Verbatim closes, so the profile runtime writes its profile, but in other
builds Verbatim's exit also closes the host's kill-on-close job, and the
kill can come first. In a build instrumented for coverage the jobs of
`verbatim-process` do not kill on close, and dropping a `Contained`
terminates its job instead, so every ending Verbatim makes while it
runs is unchanged. Only Verbatim's own exit, which drops nothing,
differs: it closes the host's pipe and kills nothing, so the host always
exits after Verbatim and writes its profile. An outpost or the listener
still running then ends the same way, through its own end on the closed
pipe. A host Verbatim ends while it runs, one lost or replaced by a
switch of synthesizer, is killed and writes no profile.

The mockapp windows that the tests and scenarios end by killing them
write no profile; their lines count only where a test runs the same code
inside its own process.

LLVM's continuous mode, which keeps the counts in the file as the process
runs and so would survive any ending, was measured and rejected: on
Windows a hot loop ran 90 times slower with it, and the outpost's text
conversion test 10 times slower (76 seconds against 7.8).

## A coverage run on GitHub

Start it by hand, on `main` or with `--ref <branch>`:

```
gh workflow run ci.yml -f coverage=true
```

The run's jobs are the ordinary ones, run differently (the comments in
`.github/workflows/ci.yml` give the details):

- The `ci` job runs `cargo xtask coverage` instead of `cargo xtask ci`
  and uploads `target/coverage` as the `coverage-unit` artifact, when a
  test failed too. The ARM64 and Linux jobs do not run. Its build cache
  and the `e2e` groups' have a key of their own in a coverage run, so the
  instrumented build is never restored by a push run.
- The `plan` job runs `cargo xtask e2e-plan --coverage`, which allows for
  the instrumented build's slowdown, so the run has more groups than a
  push's (`docs/tooling/e2e.md`, "How CI runs the suite").
- Each `e2e` group builds with the environment `cargo llvm-cov show-env`
  prints, so Verbatim, its outposts, the listener, the synthesizer host
  and mockapp are all instrumented, and every process writes a profile of
  its own (`%p` and `%m` in the name) to `target/coverage-profiles`, but
  for those ended from outside (above). After the suite, passing or
  failing, `cargo xtask coverage report e2e-<group>
  target/coverage-profiles` reports them, leaving out the harness's own
  crates (`verbatim-e2e`, `verbatim-agent` and xtask), and the group
  uploads the report as `coverage-e2e-<group>` and the profiles as
  `profiles-e2e-<group>`.
- The `coverage` job, on Linux, runs once every group has finished, failed
  ones included: `cargo xtask coverage merge` merges the groups into the
  e2e level and all levels into the merged one, genhtml writes their HTML
  reports, and the job uploads everything as `coverage-report` and puts
  the summary on the run's page. A level whose job failed before
  uploading it is named as missing in the summary, and the rest is made
  from the levels that are there.

A run that sets both `coverage` and `scenario` fails in the `plan` job,
since a coverage run measures the whole suite.

Download the report with `gh run download <run id> -n coverage-report`.
It holds:

- `summary.txt`: each level's line coverage, and the merged one's.
- `unit`, `mockapp`, `faults`, `e2e` and `merged`: each level's HTML report, in its
  `html` folder, and its lcov file beside it, as `<level>.lcov`.
- `e2e-only.txt`: each file with lines only the end-to-end suite runs, as
  the number of those lines then the path, the most first. These are the
  logic that should get unit, mockapp or fault tests.
- `unreached.txt`: each file of which no level runs a line.

## What the end-to-end level shows

It shows which lines Verbatim's own processes ran while the suite's
scenarios ran on GitHub's runner. It cannot show:

- Whether a line's behaviour was checked: a scenario asserts what is
  spoken, so a line it ran may be wrong without failing it.
- What the scenarios the CI suite leaves out run: the local-only
  scenarios, the demonstrations and the measure set
  (`docs/tooling/e2e.md`).
- What mockapp's windows run, and what a synthesizer host Verbatim ends
  while it runs ran, since they are killed and write no profile (above).
- Code in processes that are not Verbatim's: the applications the
  scenarios drive, such as Notepad, the console host and Windows Terminal.
- Code no instrumented binary contains, such as code for another
  platform, which appears in no level's report and so in neither list.
- Timing as a push run has it: the instrumented build is slower, so a
  scenario that depends on timing may fail in a coverage run alone. Its
  group still reports the profiles it wrote.
