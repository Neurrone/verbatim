# Merged review: xtask and the development tools (2026-11-20, at ba89dca7)

Merged from tools-A.md (reviewer A) and tools-B.md (reviewer B). Every quotation, count and commit below was checked by the merger against the tree at ba89dca7 with `sed -n`, `grep`, `wc -l`, `md5sum`, `diff` and `git show --stat`; nothing was built or run. This area speaks nothing, so every "speech changes" cell is no. Terms: a "scan" is one of the text checks `cargo xtask ci` runs over source before it compiles anything; "the suite" is the end-to-end scenarios GitHub's `e2e` job runs.

## 1. Size

### Settled table

Product lines are a file's lines before its first `#[cfg(test)]`; files under `tests/` count as test lines.

| Part | Product | Tests | NVDA counterpart (pinned `nvda/`, `wc -l`) | Ratio |
|---|---|---|---|---|
| Scans and the ci driver: `main.rs` 644, `tiers.rs` 533, `waits.rs` 274, `shapes.rs` 158, `ownership.rs` 1,137, `docs.rs` 598 | 3,344 | 803 | `tests/checkPot.py` 204 (NVDA lints with Ruff and pyright, no hand-written scans) | not like for like |
| CI orchestration: `coverage.rs` 811, `plan.rs` 374, `group.rs` 157, `durations.rs` 283, `.github/workflows` 1,106 | 2,731 | 505 | `sconstruct` 754, `site_scons/` 528, `run*.bat` 89, `ci/` 849, `.github/workflows/` 1,608 | see below |
| Local harness drivers: `local_e2e.rs` 182, `agent.rs` 77, `contain.rs` 50, `demo.rs` 232, `nvda.rs` 1,005, `perf.rs` 580, `trace.rs` 197, `park.rs` 163, `replay.rs` 22 | 2,508 | 377 | none | none |
| Hyper-V harness: `xtask/src/vm/` | 2,478 | 268 | none | none |
| Terminal conformance tools: `conformance.rs` 647, `record.rs` 486 | 1,133 | 0 | none | none |
| Tool crates: event-record 2,833, trace-waits 2,103, replay 659 (604 + `build.rs` 55), inspect 544, synth-capture 195 | 6,334 | 1,288 | none | none |
| Rust total | 17,422 | 3,241 | | |
| `vm/scripts` (PowerShell) | 996, of which 696 serve only the VM | | | |
| `.config/` | 75 | | | |

NVDA's build, lint and CI tooling recounts to 4,032 lines (754 + 528 + 89 + 849 + 1,608 + 204). Reviewer A's 4,030 was right; B's 3,750 was not.

Ratios, settled: like for like (scans, the ci driver and CI orchestration, 6,075 lines) is 1.5 times NVDA's 4,032; the whole area (17,422 Rust, 1,106 YAML, 996 PowerShell, 19,524 in all) is 4.8 times. B's 2.6 and 5.2 used the wrong NVDA total and counted the local drivers as like for like. B's event-record figure of 2,346 product lines was wrong (`wc -l` gives 3,320 lines, 487 of them tests, so 2,833 product); B's own Rust total of 17,422 is right. Reviewer A's xtask test figure of 2,129 was wrong; xtask has 1,953 test lines.

What the extra lines buy: the tier and determinism rules (licence tiers and replayable reducers, CLAUDE.md "NVDA provenance" and `books/dev/src/reference/testing.md` "The crates on a reducer's path"); flight-recorder replay; ETW trace reading; the event recorder (the CLAUDE.md standing rule on recordings of real applications); the NVDA capture and compare tools; and a Hyper-V harness nothing in the standing rules runs.

### Deletion target

| Part | Product (Rust) | Tests | Other |
|---|---|---|---|
| Remove now | about 415 | about 80 | 165 YAML (`vm-smoke.yml` 125, a composite cache action about 40) |
| If Dickson rules for the removal | about 245 (planner parse 145, recorder passes 100), and 2,478 if the VM harness goes | about 140 (planner tests 90, passes 51), and 268 with the VM harness | 133 YAML (`explore.yml`), about 45 PowerShell in `ci.yml`; with the VM harness 696 PowerShell, 344 Packer and answer-file lines, `vm/README.md` 134, `books/dev/src/how-to/vm.md` 284 |
| Planned phase | none: `conformance.rs` and `record.rs` (1,133) feed M6.4's conformance corpus (`docs/roadmap.md:1308`, under "M6.4 — Terminal reading rebuild and residuals"); no finding targets them | | |

## 2. Findings

| id | title | class | found by | file and line | what goes | product saved | test saved | speech changes |
|---|---|---|---|---|---|---|---|---|
| T1 | `cargo xtask e2e` reports a misspelled scenario as passed | Defect | B only | `xtask/src/local_e2e.rs:154-181`, crates/verbatim-e2e/tests/e2e.rs:284,292 | nothing; a guard of about 8 lines is added | -8 | -10 | no |
| T2 | `cargo xtask measure` exits 0 when a scenario failed or ran nothing | Defect | merger, from point 1's check | xtask/src/perf.rs:145-147,192-246 | nothing; the run fails when any line says FAILED | -3 | 0 | no |
| T3 | The measure set's default list has drifted from the registry | Defect | A only | `xtask/src/perf.rs:29-37` | `MEASURE_SET` and the count in `measuring.md:36` | 9 | 0 | no |
| T4 | The nightly Explore workflow runs zero tests and mutates deleted files | Question for Dickson | both | `.github/workflows/explore.yml:37-43,64` | the workflow or its dead parts; `testing.md:122-124` | 0 | 0 (133 YAML) | no |
| T5 | Dead exception entries for the deleted exploration tests | Remove now | both | xtask/src/ownership.rs:769-771,784-786 | six entries | 6 | 0 | no |
| T6 | `DETERMINISM_ENTRIES` is a second copy of `clippy.toml` | Remove now | both | xtask/src/ownership.rs:629-752,904-910,1417-1428 | the const, `missing_entries`, its test | about 136 | 13 | no |
| T7 | The two exception lists repeat ten paths | Remove now | both | xtask/src/ownership.rs:760,776 | one list with a kind per entry | about 10 | 0 | no |
| T8 | `cargo xtask ci` stops at the first failing step | Question for Dickson | both | `xtask/src/main.rs:363-458` | nothing; about 20 lines change | 0 | 0 | no |
| T9 | CRLF checkouts on GitHub's runners; one `.gitattributes` line | Remove now | A only (B noted the failure) | `.gitattributes:1-3`, `listener.rs:3559-3571`, `nvda.rs:131` | two CRLF branches | about 8 | 0 | no |
| T10 | Crate and binary lists that the tier graph or Cargo hold | Remove now | both (LINUX_CRATES); B only (fault lists, binary lists) | `main.rs:146-161,462-485`, `ci.yml:404`, vm/deploy.rs:518, `scenario.rs:3742` | hand lists derived | about 50 | 9 | no |
| T11 | The planner parses Rust source as text, and a second check holds the parse | Question for Dickson | both | `xtask/src/plan.rs:127-288`, `group.rs:88-110` | the parser and its agreement checks | about 145 | about 90 | no |
| T12 | `demo` and CI start the agent their own way | Remove now (demo); Question (CI) | both | `demo.rs:133-160,206-232`, `ci.yml:494-534` | demo's agent code; CI's PowerShell loop | about 55 | 0 | no |
| T13 | Scan helpers written two and three times, and a source rule outside the scans | Remove now | both | `waits.rs:125,145`, `ownership.rs:408,458,498,973`, `coverage.rs:493`, `listener.rs:3559-3611` | one walker, one brace finder, one test-module finder; the listener's text test | about 50 | about 55 | no |
| T14 | `vm-smoke.yml`, retired by the roadmap, is still there | Remove now | both | `.github/workflows/vm-smoke.yml` | the file | 0 | 0 (125 YAML) | no |
| T15 | `source_commit` copied between two build scripts | Remove now | both | `verbatim-replay/build.rs:19`, `verbatim-app/build.rs:85` | one copy, to `build-support/` | 37 | 0 | no |
| T16 | `HOOK_START_FORBIDDEN` restates `HOOK_FORBIDDEN` | Remove now | B only | `ownership.rs:263-290,298-323` | 23 repeated entries | about 20 | 0 | no |
| T17 | Coverage's fault steps rebuild `ci`'s fault arguments | Remove now | A only | `coverage.rs:115-139`, `main.rs:492-530` | one builder | about 20 | 0 | no |
| T18 | The event recorder names its UIA events in two lists | Remove now | A only | `verbatim-event-record/src/uia_events.rs:42-68,370-401` | one table | about 20 | 0 | no |
| T19 | The waits scan prints production sleeps nothing acts on | Remove now | A only | `xtask/src/waits.rs:85-95` | the print and its doc lines | about 10 | 0 | no |
| T20 | Workflow cache steps written three times; checks repeated on three hosts | Remove now (composite action); Question (skipping checks) | both | `ci.yml:119-137,206-224,381-399` | one composite action | 0 | 0 (about 40 YAML) | no |
| T21 | Stale descriptions of the checks and commands | Remove now (books, Cargo.toml); Question (CLAUDE.md) | both | `setup.md:179-187`, `xtask/Cargo.toml:3`, `CLAUDE.md:123` | the stale text | 0 | 0 | no |
| T22 | The pure rule's forbidden names overlap `clippy.toml` | Question for Dickson | A only | `ownership.rs:160-182,329-347` | the forbidden lists' overlap, not the allow-lists | about 0 net | about 0 | no |
| T23 | The event recorder numbers passes for a design not in the repository | Question for Dickson | both | `verbatim-event-record/src/passes.rs`, `hooks.rs:26-28` | `passes.rs`, the pass field (format 2) | about 100 | 51 | no |
| T24 | The Hyper-V harness has no routine user | Question for Dickson | both | `xtask/src/vm/`, `vm/` | the harness | 2,478 | 268 | no |

### T1. `cargo xtask e2e` reports a misspelled scenario as passed (Defect, B only, confirmed)

`xtask/src/local_e2e.rs:160-180` runs `cargo test -q -p verbatim-e2e --test e2e -- --ignored --exact <scenario> --test-threads=1` and returns `Ok(status.success())`; line 103 then prints `"run {run} {scenario}: pass in {seconds} s ({})"`. The test binary filters its own trials, `crates/verbatim-e2e/tests/e2e.rs:284` `trials.retain(|trial| command_line::selected(trial.name(), &filters, arguments.exact));`, and ends with `conclusion.exit_code()` (line 300). In libtest-mimic 0.8.2 (the version in `Cargo.lock:970-971`, source read from the cargo registry), `exit_code` returns `ExitCode::SUCCESS` unless `has_failed()`, which is `self.num_failed > 0` (`lib.rs:384-395`); `run` with an empty list fails nothing.

Sequence: `cargo xtask e2e conhost_typng` builds, starts the agent, runs zero trials, prints `run 1 conhost_typng: pass in N s`, and exits 0. This is the one local path CLAUDE.md allows ("Run locally only the one to three end-to-end scenarios ... through `cargo xtask e2e`").

Every command that runs scenarios by name, checked:

| Command | Can zero scenarios report success? | Why |
|---|---|---|
| `cargo xtask e2e` | yes, exit 0 and "pass" | no check of the name (above) |
| `cargo xtask measure` | the line says FAILED, but the command exits 0 | `passed` (`perf.rs:145-147`) needs `"test result: ok. 1 passed"`, which a zero-trial run does not print (libtest-mimic prints `test result: ok. 0 passed`, `printer.rs:217-229`); `run_measure` returns `Ok(())` whatever the results (T2) |
| `cargo xtask trace scenario` | no | the same `passed`, and `run_trace_scenario` returns `Err(format!("{name} failed; see its log"))` |
| `cargo xtask demo` | no | `parse` refuses a name the registry lacks (`demo.rs:103-104`, `registry::find(&scenario).ok_or_else(...)`), and a passing run with no video fails (`"{scenario} passed but made no video at {}; is ffmpeg on PATH?"`) |
| `cargo xtask nvda compare` | only for a registered scenario with no test, which the registry's test `the_test_binary_has_a_test_for_every_scenario_and_no_other` (`registry.rs:2863`) forbids; none today (`DISABLED` is empty, `registry.rs:239`) | `nvda.rs:592-593` refuses an unregistered name |
| `cargo xtask vm test` | no | a clean exit with no run summary is a failure (vm/test.rs:324-330) |
| CI, repeat by hand | no | `ci.yml:524-528` lists first and fails unless exactly one test is listed |
| CI, a group | no | the names are the binary's own listing (`group.rs:114-148`, `list_tests`) |

Reviewer B's claim that `demo` and `nvda compare` have no guard is wrong; its claim for `measure` holds at the exit code only. Fix: in `e2e.rs`'s `main`, fail when filters were given and no trial is left (about 8 lines and a hand-written test of the selection); the CI list check could then go.

### T2. `cargo xtask measure` exits 0 when a scenario failed (Defect, merger)

`xtask/src/perf.rs:213-221` writes `"{name}: FAILED"` and continues; `run_measure` ends `Ok(())` (line 245) with no look at the results, and `measure` maps `Ok(())` to `ExitCode::SUCCESS` (line 184). Sequence: `cargo xtask measure --scenario measure_keys_txt_box` prints `measure_keys_txt_box: FAILED`, writes an empty stages report and exits 0. Fix: return an error naming the failed scenarios after the report is written (about 3 lines). Found while checking T1's command list; neither reviewer reported it.

### T3. The measure set's default list has drifted (Defect, A only, confirmed)

`xtask/src/perf.rs:27-37`: `/// The measure set's scenarios a silent run takes; `measure_speech_real` needs a real device and an audible run, so it is asked for by name.` followed by 7 names. The registry places 16 scenarios in `Group::Measure` (awk over `registry.rs`), and `crates/verbatim-e2e/tests/e2e.rs:173-190` lists the same 16. The 8 left out of the default are `measure_focus_return`, `measure_focus_return_retitled`, `measure_focus_flood`, `measure_keys_notepad`, the three `measure_stale_caret_*` and `measure_held_repeat_text_box`. `books/dev/src/how-to/measuring.md:36` says "the eight `measure_` scenarios". `MEASURE_SET` was written in 8511cd19 (2026-10-13) and not changed since; the stale-caret scenarios came in c04584db (2026-10-22). Sequence: `cargo xtask measure` with no `--scenario` runs 7 of the 15 silent measurements. Fix: derive the default from `registry::SCENARIOS` in `Group::Measure` less `measure_speech_real`; delete the const and the count in the guide.

### T4. The nightly Explore workflow runs zero tests (Question for Dickson, both)

`.github/workflows/explore.yml:17` `- cron: "17 3 * * *"`; line 64 `cargo test --release "${packages[@]}" --lib -- --ignored explore_`. `grep -rn "fn explore_\|VERBATIM_EXPLORE" crates xtask uia-facts --include=*.rs` finds 0 lines, so every night the filter matches nothing and the job passes. The `mutants` job's `GROUPS` (lines 37-43) names, with the commit that deleted each (`git log -1 --diff-filter=D`):

| Stale name in `explore.yml` | Deleted in |
|---|---|
| crates/verbatim-reader-rules/src/in_flight.rs | 93b2974c (2026-11-16) |
| crates/verbatim-outpost-logic/src/gap_record.rs | adb0a7e2 (2026-11-07) |
| crates/verbatim-outpost-logic/src/verdicts.rs | b842c487 (2026-11-09) |
| crates/verbatim-outpost-logic/src/hold.rs | 1e6c10fb (2026-11-16) |
| crates/verbatim-core/src/queries.rs | 3f9e68ac (2026-11-17) |
| function filters on `reduce.rs`: `leaves_focus` | efad6825 (2026-11-08), last change by `git log -S` |
| `observed_before_foreground_change`, `outpost_ended` | 3f9e68ac (2026-11-17) |
| function filters on `terminal_model.rs`: `ask_model`, `model_asks`, `answered_read`, `reply`, `answerable` | 89c36cc8 (2026-11-16) |

Of nine function filters only `foreground_changed` names a function today (`reduce.rs:213`). The exploration test files themselves, `in_flight/tests/explore.rs` (93b2974c), `outpost-logic/src/tests/explore.rs` (2886e858) and `core/src/queries/explore.rs` (3f9e68ac), are gone. Neither reviewer named adb0a7e2, b842c487 or 1e6c10fb; reviewer A's list attributed all deletions to the three exploration commits. `books/dev/src/reference/testing.md:124` still says: "This is the recorded decision for those ignored tests: only the scheduled workflow (`.github/workflows/explore.yml`) runs them". Changed fact: the tests it governs were deleted on 2026-11-16 and 17. It is a question because removing the job, or pointing `mutants` at today's owners, changes what CI runs. Sequence: any night, the `explore` job reports success with no case run, and `mutants` runs on the four groups whose files remain (`lib.rs`, `watch.rs`, `follow.rs`, `intake.rs`, `schedule.rs`, `foreground.rs`, `reduce.rs` filtered to one function, `terminal_model.rs` filtered to none). Side note outside the area, confirmed: `crates/verbatim-core/Cargo.toml:11` lists `proptest.workspace = true` under `[dependencies]` and no Core source names proptest.

### T5. Dead exception entries (Remove now, both)

`xtask/src/ownership.rs:769-771` and `784-786` each list `"crates/verbatim-reader-rules/src/in_flight/tests/explore.rs"`, `"crates/verbatim-outpost-logic/src/tests/explore.rs"` and `"crates/verbatim-core/src/queries/explore.rs"`; all three files are gone (T4). Nothing fails on an exception naming no file. Delete the six entries.

### T6. `DETERMINISM_ENTRIES` is a second copy of `clippy.toml` (Remove now, both)

`xtask/src/ownership.rs:629` `const DETERMINISM_ENTRIES: [&str; 122] = [` (to line 752), used only by `missing_entries` (lines 904-910) and its test `a_clippy_toml_missing_an_entry_is_found` (1416-1428): grep over `crates/`, `xtask/`, `uia-facts/` finds no other use. The canonical file's header (`crates/verbatim-core/clippy.toml:5-6`): "Each covered crate keeps an identical copy of this file, which `cargo xtask ci` checks, since Clippy has no include." `md5sum` gives eight identical 134-line files (core, input, listener-logic, model, outpost-logic, reader-rules, term-sim, text); `crates/verbatim-outpost/clippy.toml` is a different 8-line file for the outpost's lock rule. Equality is checked at `ownership.rs:866` `Ok(text) if text == canonical => {}`. What the Rust list adds: it fails if an entry is deleted from all eight files together, an edit any diff shows. Adding a disallowed method touches nine places. Rule asking for the check: `testing.md:99`, "The ownership scan (`xtask/src/ownership.rs`) keeps those files equal, forbids silencing them outside its listed exceptions, forbids the whole modules that Clippy names function by function"; it does not ask for a second list. Removes about 136 product and 13 test lines. Reviewer A's 145 and B's 135 bracket the recount.

### T7. The two exception lists (Remove now, both)

`EXPECT_EXCEPTIONS: [&str; 11]` (line 760) and `MODULE_EXCEPTIONS: [&str; 10]` (line 776) differ only by `crates/verbatim-model/src/lib.rs`; used at `ownership.rs:1010-1011`. The month's only two commits that edited them, c5c0181a (2026-11-18, "List the validity replays' test among the determinism rule's exceptions", only `ownership.rs`) and b43b3c2d (2026-11-19, which also changed a shape count in `performance.md` and `shape/tests.rs`), each added one path to both. One list of paths with a kind per entry saves about 10 lines.

### T8. `cargo xtask ci` stops at the first failing step (Question for Dickson, both)

The code, `xtask/src/main.rs:368-374`:

```
let covered = match tiers::check(root) {
    Ok(covered) => covered,
    Err(error) => {
        eprintln!("xtask ci: step failed: crate tiers\n{error}");
        return ExitCode::FAILURE;
    }
};
```

each later scan ends `return ExitCode::FAILURE;` the same way, and `run_steps` (lines 438-458) is documented "Runs each of `steps` in turn as `command`'s, stopping at the first that fails." and returns on the first failed status.

Steps in order on Windows, with what each needs:

| Step | Compiles? |
|---|---|
| crate tiers (`cargo metadata`) | no |
| waits in test code | no (text) |
| UIA cached reads through shapes | no (text) |
| pure owners' names | no (text) |
| determinism | no (text and `clippy.toml`), but takes `covered` from the tier step (`ownership::determinism(root, &covered)`, line 395) |
| documents' links and paths | no (text) |
| `hakari generate --diff`, `hakari manage-deps --dry-run` | no (metadata) |
| `fmt --all --check` | no |
| `clippy --workspace --all-targets -- -D warnings` | yes (check build) |
| three `nextest run` steps (all, `ALONE_TESTS`, `SERIAL_TESTS`) | yes (test build) |
| `test --workspace --doc` | yes |
| seven `clippy -p X --features faults` | yes, rebuilds each with the feature |
| seven `test -p X --features faults --test T` | yes |

On Linux: the six scans, then one `clippy` over `LINUX_CRATES`, then the coverage gate. Every step could run regardless of another's failure except determinism, which needs the tier step's covered set; `tiers::check` computes it (`tiers.rs:517`) before judging offences, so returning it alongside the offences would remove that dependency too. Both reviewers said no step depends on another: true except for that one. A compile error fails Clippy and the tests both. nextest already runs with `fail-fast = false` (`.config/nextest.toml:37`). The only timings recorded: the three nextest steps about 70 s and doctests 9 s with nothing to rebuild (`books/dev/src/how-to/setup.md:136-139`); the coverage command 5.5 minutes uncached (`coverage.md:63`). It is a question because it changes what CI runs after a failure.

### T9. CRLF on GitHub's runners (Remove now, A only, confirmed)

`.gitattributes` as it stands:

```
vm/vendor/ffmpeg/*.exe filter=lfs diff=lfs merge=lfs -text
videos/**/*.mp4 filter=lfs diff=lfs merge=lfs -text
sounds/*.wav binary
```

Every job's checkout is `- uses: actions/checkout@v7` with no line-ending setting (`ci.yml:59,93,105,191,266,309,344,672`; no `autocrlf` or `eol` anywhere in `.github/`). This machine has `core.autocrlf=input` and `core.eol=lf` from `C:/Users/Dickson/.gitconfig`; `git ls-files --eol` shows every tracked text file as `i/lf`, none CRLF. The one commit fixing a CRLF checkout failure (searched: `--grep='crlf\|line ending\|checkout\|carriage'`): 02b2d5d7 (2026-11-19), "GitHub's Windows runners check out with CRLF, so the test's search for a closing brace followed by LF never matched and every CI run since b70f5bb7 failed on it." Code branches that exist only for CRLF checkouts: `crates/verbatim-listener/src/listener.rs:3566-3569` (`.find(|&at| matches!(rest.as_bytes().get(at + 2), Some(b'\n' | b'\r')))`, added by 02b2d5d7; reviewer A's 3509-3518 has drifted) and `xtask/src/nvda.rs:131` `entries.push((name, text.replace("\r\n", "\n").into_bytes()));`, from 18c6b771, which keeps the committed add-on package byte-identical on any checkout. Not CRLF-checkout branches: `conformance.rs:441-443` parses a terminal's output and vm/connect.rs:174-181 writes an RDP file. Other source-reading code (the scans, `plan.rs`, `event_queue/tests.rs:1238-1243`, `registry.rs:2848-2860`) splits with `str::lines` or on tokens that do not end a line, and is safe today by that. Proposal: `* text=auto eol=lf`; since the index is all LF, nothing is renormalized, and the two branches go. To be told to Dickson as a tooling decision of Claude's.

### T10. Hand lists the tier graph or Cargo already hold (Remove now; LINUX_CRATES both, the rest B only, confirmed)

`xtask/src/main.rs:146-161` `LINUX_CRATES` is the nine platform-neutral and three logic crates of `tiers.rs` `TIERS` plus `verbatim-replay` and `xtask`; `coverage.rs:337-344` `gate_crates()` derives the same twelve from `TIERS`; `every_gated_crate_is_tested_on_linux` (`coverage.rs:991-999`) holds them together and pins `assert_eq!(gate_crates().len(), 12);`. `FAULT_CRATES` (`main.rs:462-470`) is exactly the seven manifests with a `faults` feature, and `FAULT_TESTS` (`main.rs:477-485`) is the six targets with `required-features = ["faults"]` plus `("verbatim-audio", "mixer")`, which needs naming. The suite's binaries are listed at `ci.yml:404` (B said 403), `xtask/src/vm/deploy.rs:518-536` and `crates/verbatim-e2e/src/scenario.rs:3742-3764`; 0ff2864e edited all three to add `verbatim-event-record`. About 50 product and 9 test lines.

### T11. The planner reads Rust source as text (Question for Dickson, both)

`xtask/src/plan.rs:176` `registry_entries` finds `"pub const SCENARIOS: &[ScenarioDef] = &["` and reads lines with `line.strip_prefix("name: \"")`, `"group: Group::"`, `"local_only: "`; `test_names` (line 236) reads `tests/e2e.rs` for lines ending `scenario_tests! {`; `suite_from_source` (270-287) applies the suite rule. `group.rs:88-110` `compare` fails a Windows group when the binary's listing differs, and `the_planners_reading_of_the_registry_is_the_registry` (`group.rs:163`) checks the parser against the real file. The header says why (`plan.rs:4-5`): "the `plan` job runs it on Linux, where xtask builds neither the registry crate nor the test binary." The suite rule (not Demo, not Measure, not local-only) is written at `plan.rs:283`, `group.rs:153-156` and the registry's default selection (`registry.rs:1842-1846`). The merger also found the scenario names written twice inside the e2e crate: `tests/e2e.rs:48-190` lists every name again in `scenario_tests!` blocks although each trial only calls `registry::run_named(name)` (`e2e.rs:230`), held to the registry by `the_test_binary_has_a_test_for_every_scenario_and_no_other`, which reads `e2e.rs` as text. Both reviewers' replacements (the plan reads only `xtask/e2e-durations.txt`, and a group over budget fails on Windows) add a failure mode: a new scenario not yet in the table is counted by `group.rs` but not by the plan, so a group can exceed its 10-minute step until `cargo xtask e2e-durations` is rerun, and that table needs the scenario's GitHub run first. It changes the `plan` job, so it is a question. Removes about 145 product lines (`read_suite`, `registry_entries`, `test_names`, `suite_from_source`, `compare`) and about 90 test lines (`plan.rs:478-539`, two tests in `group.rs:162-191`).

### T12. `demo` and CI start the agent their own way (Remove now for demo; Question for CI; both)

`xtask/src/agent.rs:41-69` refuses a busy port, logs, and waits on the port or the agent's exit. `demo.rs:133-160` builds and spawns its own on port 44,002, `wait_for_port` (212-221) polls every 200 ms, and `struct KillOnDrop(Child);` (224) ends it. 60bd5146 (2026-11-19) moved `measure` onto `agent.rs` and left `demo`. Merger's check: `Agent::start` runs `root.join(r"target\debug\verbatim-agent.exe")` (`agent.rs:50`) although `agent::build` builds into `CARGO_TARGET_DIR` when it is set, while `demo`'s `target_dir` honours it (`demo.rs:206-209`, tested by `the_agent_comes_from_cargo_target_dir_when_set`); with `CARGO_TARGET_DIR` set, `cargo xtask e2e`, `measure` and `nvda` start a stale or missing agent. So `demo` moves onto `Agent` once `Agent` takes a port and the target directory, and the target-directory test moves with it (B's count of 14 test lines removed is wrong). About 55 product lines. The CI step (`ci.yml:494-534`) starts the agent in PowerShell with `Start-Sleep -Milliseconds 500` in a 40-attempt loop and repeats scenarios in its own loop; replacing it with `cargo xtask e2e` changes the CI step and is part of the CI question.

### T13. Scan helpers written several times (Remove now, both)

Brace matchers: `waits.rs:145` `cfg_test_lines`, `ownership.rs:458` `blank_test_modules`, `ownership.rs:498` `keep_only_function`. Test-module finders: `ownership.rs:408` (only `mod name;`, mapped to `{name}.rs`) and `coverage.rs:493` (also `pub mod` and `pub(crate) mod`, and the module's folder), one rule written twice and already different. Walkers: `waits.rs:125` `collect_rust_files`, `ownership.rs:973` `rust_files`. The listener test `the_event_path_reads_windows_only_through_its_event_windows` (`listener.rs:3578-3611`) with its `body` helper (3559-3571) is the per-function rule `ownership.rs` already has (`within: Some("hook_thread")`); as four `Checked` entries (about 12 lines) it drops about 55 test lines and its CRLF branch. About 50 product lines net.

### T14. `vm-smoke.yml` (Remove now, both)

`docs/roadmap.md:2105-2106`, under "Ongoing tracks (every milestone)": "`.github/workflows/vm-smoke.yml`, which probed nested virtualization on larger runners, is to be retired." The file (125 lines) is still there; its last change is a7c954e9 (2026-11-04), a path rewrite. Deleting it carries out the recorded entry.

### T15. `source_commit` copied (Remove now, both)

`diff` of `fn source_commit` in `crates/verbatim-replay/build.rs:19` and `crates/verbatim-app/build.rs:85` finds no difference; each is 37 lines. `build-support/artifact_dir.rs` already shares build-script code. About 37 lines.

### T16. `HOOK_START_FORBIDDEN` restates `HOOK_FORBIDDEN` (Remove now, B only, confirmed)

`ownership.rs:263` `HOOK_FORBIDDEN: [&str; 26]` and `:298` `HOOK_START_FORBIDDEN: [&str; 24]`; the second is the first without `mpsc`, `std::io` and `tracing` and with `GetMessageW`, as its doc says. They share 23 entries (B said 22). Writing it as that difference saves about 20 lines and removes the drift.

### T17. Coverage's fault steps (Remove now, A only, confirmed)

`coverage.rs:119-139` builds `"llvm-cov", "test", "--no-report", "-p", package, "--features", "faults", "--test", test` over `FAULT_TESTS`, the same arguments `main.rs:516-526` builds without the prefix. One builder with a prefix saves about 20 lines.

### T18. The recorder's UIA event names (Remove now, A only, confirmed)

`uia_events.rs:42-68` registers 3 desktop and 18 target events; `event_name` (370-405) names the same 21 again and falls back with `.map_or("Unknown", |(_, name)| name)`, a default reachable only if the lists drift. One table of (id, name, scope) removes the second list and the fallback; about 20 lines.

### T19. The waits scan's production-sleep print (Remove now, A only, confirmed)

`waits.rs:85-95` prints `"xtask ci: fixed sleeps in production code, for review:"` and the list on every run; nothing fails or records on it. Delete the print and its mention in `setup.md` (about 10 lines), or make it a failure with an allow-list.

### T20. Workflow repeats (Remove now for the composite action; Question for the rest; both)

The wxWidgets and eSpeak NG cache steps are written in `ci` (`ci.yml:119-137`), `ci-arm64` (206-224) and `e2e` (381-399). A composite action holds them once (about 40 YAML lines) and changes nothing that runs. The six scans run identically in `ci`, `ci-arm64` and `ci-linux` because `ci()` runs them before any step on every host; running them only in `ci-linux`, and `uia-facts.yml`'s trigger on `.github/workflows/ci.yml` (line 35), change what CI runs.

### T21. Stale descriptions (Remove now for books and the manifest; Question for CLAUDE.md; both)

`books/dev/src/how-to/setup.md:179-187` describes the pure-owners scan as the read scheduler alone with an allow-list ("`std::collections`, `std::time::Duration`, `verbatim_model` and the listener's data types") that is not `ownership.rs:70-85`'s, and omits the outpost reducer, hook, audio and determinism rules. `xtask/Cargo.toml:3` names five commands. `CLAUDE.md:123` tells the reader to start the agent by hand and run `cargo test` directly, against the rule of 2026-11-19 (8097dfce) in the same file: "Run locally only the one to three end-to-end scenarios whose code path a change touches, through `cargo xtask e2e`". CLAUDE.md is Dickson's ("The working process: this file and `REVIEW.md`").

### T22. The pure rule and `clippy.toml` (Question for Dickson, A only, reclassified)

`ownership.rs:160-182` `REDUCER_FORBIDDEN` and `:329-347` `FORBIDDEN` name `"Instant"` or `"Instant::now"`, `"SystemTime"`, `"std::fs"`, `"std::env"`, `"std::process"`, `"std::net"`, `"windows::"` and `"unsafe"`, which the determinism `clippy.toml`, the tier check and (for `unsafe`) a lint could hold; `verbatim-outpost-logic` and `verbatim-listener-logic` lack `#![forbid(unsafe_code)]` (grep of every `lib.rs`). Reviewer A's claim that the rule then "becomes the compiler's" is wrong: each `Checked` entry also has an allow-list (`allowed`, lines 70-84 and 139-153), a rule Clippy cannot express, and `ownership.md:24` records that as the enforcement: "an ownership check in `cargo xtask ci` reads the reducer modules against an allow-list of paths, with no binding, clock, thread, channel or lock". Adding `Mutex` and the atomics to the shared `clippy.toml` would also fail `verbatim-model`, which uses them in `lib.rs` and `protocol/responder.rs`. What moves is about 45 lines of lists, with no net saving; it is a question because it changes a recorded enforcement.

### T23. The recorder's passes (Question for Dickson, both)

10643208 (2026-11-18): "The focus-batch design needs recordings showing which drain pass of the receiving thread each WinEvent arrived in." It added `passes.rs` (103 lines, 52 product and 51 test) and reworked `hooks.rs` (187 lines changed). `grep -ril "focus.batch\|focus batch\|drain pass" docs books` finds only `books/dev/src/reference/crates/verbatim-event-record.md`; no design or roadmap entry. `hooks.rs:26-28` copies `verbatim-ia2`'s private `EVENT_THREAD_PRIORITY` (`event_thread.rs:60`) as `const WINEVENT_PRIORITY: ThreadPriority = ThreadPriority::Highest;`.

### T24. The Hyper-V harness (Question for Dickson, both; laid out, not recommended)

See section 4.

## 3. Test findings

| Crate | Tests' lines | Removable | Replaceable | To add |
|---|---|---|---|---|
| xtask | 1,953 | `a_clippy_toml_missing_an_entry_is_found` (13, T6); `every_gated_crate_is_tested_on_linux` (9, T10); if T11 is ruled for, `the_suite_is_read_from_the_registry_and_the_tests_file` (61), `the_planners_reading_of_the_registry_is_the_registry` and `a_suite_unlike_the_planners_fails_naming_the_difference` (30) | the walkers' tests fold into one walker's (T13); `the_agent_comes_from_cargo_target_dir_when_set` moves to `agent.rs` (T12) | `measure` failing on a failed scenario (T2), about 10 |
| verbatim-listener | | `the_event_path_reads_windows_only_through_its_event_windows` and `body` (about 55, T13) | four `Checked` entries in `ownership.rs` | |
| verbatim-e2e | | | | the selection guard of T1, about 10 |
| verbatim-event-record | 487 | `passes.rs`'s tests (51) if T23 is ruled for | | |
| verbatim-replay (369), verbatim-trace-waits (322), verbatim-inspect (110) | 801 | none found | | |
| workflows | | `explore` job, zero tests nightly (T4) | | |

Both reviewers found nothing removable in replay, trace-waits and inspect; the merger's read agrees (replay calls `verbatim_core::replay` and only renders; inspect uses `verbatim_control::protocol`'s types directly).

## 4. Questions for Dickson

### Q1. The nightly Explore workflow (T4)

Decision of record, `testing.md:124`: "This is the recorded decision for those ignored tests: only the scheduled workflow (`.github/workflows/explore.yml`) runs them". Changed fact: the exploration tests were deleted in 93b2974c, 2886e858 and 3f9e68ac (2026-11-16 and 17), and five of the mutated files earlier or alongside. Sequence today: every night `explore` passes with no case run; `mutants` runs on the four groups whose files remain, with eight of nine function filters naming nothing. Options: delete the workflow and the testing.md section (133 YAML lines); or keep `mutants` on files that exist and delete `explore`. Speech: none.

### Q2. CI changes (T8, T11, T12's CI half, T20's skipped checks)

None changes a decision of record; each changes what CI runs. (a) `cargo xtask ci` runs every step and fails at the end with the list (determinism needs the tier step to return its covered set even with offences); sequence today: a run failing rustfmt never reaches Clippy or the tests, so four kinds of failure take four runs. (b) The plan job stops parsing `registry.rs` and `e2e.rs`, with the new failure mode in T11. (c) CI's e2e step starts the agent and repeats scenarios through `cargo xtask e2e`. (d) The scans run once, in `ci-linux`, not in all three jobs. Speech: none.

### Q3. CLAUDE.md:123 (T21)

The paragraph tells sessions to start the agent by hand and run `cargo test` directly; the standing rule of 2026-11-19 says local runs go "through `cargo xtask e2e`". Replace the paragraph with a pointer to `cargo xtask e2e`? Speech: none.

### Q4. The pure rule's forbidden lists (T22)

Recorded enforcement, `ownership.md:24`: "an ownership check in `cargo xtask ci` reads the reducer modules against an allow-list of paths, with no binding, clock, thread, channel or lock". The allow-lists cannot move to Clippy; the overlap in the forbidden lists can. Keep both, or move the overlap to `clippy.toml` and add `#![forbid(unsafe_code)]` to the two logic crates? No net line saving. Speech: none.

### Q5. The event recorder's passes (T23)

Is the focus-batch design planned? If not, `passes.rs`, the hook-thread rework's pass numbering and file format 2's `pass` field go (about 100 product and 51 test lines); if it is, the priority should be `verbatim-ia2`'s constant made public. Speech: none.

### Q6. The Hyper-V harness (T24), laid out without a recommendation

Lines: `xtask/src/vm/` 2,478 product and 268 test (`connect.rs`, `create.rs`, `deploy.rs` 622, `dotenv.rs`, `host.rs` 553, `lifecycle.rs`, `logs.rs`, `mod.rs`, `packer_build.rs`, `test.rs` 474); VM-only PowerShell 696 (`Build-VerbatimWindows11Image.ps1` 347, `Initialize-VerbatimHarness.ps1` 289, `Initialize-VerbatimBaseImage.ps1` 60); Packer files 206 (`verbatim-windows11.pkr.hcl` 97, `variables.pkr.hcl` 95, `example.pkrvars.hcl` 14) and the answer file 138; `vm/README.md` 134; `books/dev/src/how-to/vm.md` 284. Both reviewers counted all 996 lines of `vm/scripts`; 300 of them serve other commands: `Enable-VerbatimCrashDumps.ps1` (101, cited by `crates/verbatim-e2e/src/scenario.rs:139`), `Register-VerbatimParkTask.ps1` (88, `cargo xtask park`) and `Register-VerbatimTraceTask.ps1` (111, `cargo xtask trace`). The vendored `vm/vendor/ffmpeg/ffmpeg.exe` is used by `deploy.rs`.

D3 (`architecture.md:31-38`): "D3, the end-to-end suite runs wherever the harness reaches; CI is hosted runners only. The suite talks to the in-guest agent at an address, on a hosted runner, a developer's machine or VM, or a local Hyper-V VM (section 14). CI runs it silently on every change, recorded, with no self-hosted runners, no nested virtualization and never NVDA; a snapshot restore is always an explicit request. ... Decided 2026-07-13. Amended 2026-10-01."

Section 14 (`architecture.md:1086-1093`): "`HyperVHost` is the only implementation, kept for contributors who test in a local VM; the maintainer runs the suite runner-direct on a development VM instead (D3)." `xtask/src/vm/mod.rs:5-9` keeps the `Host` trait so "the deferred CI story (a QEMU/KVM host on Linux runners, per the architecture doc)" can be added, which D3's "no self-hosted runners, no nested virtualization" does not plan.

Last functional change: 0ff2864e (2026-11-11), 16 lines in vm/deploy.rs to stage `verbatim-event-record`; before it 8efb3da1 (2026-10-31), `deploy.rs` and `Enable-VerbatimCrashDumps.ps1` for the outpost split. Reviewer A's "no functional commit since 2026-10-07" is wrong. No commit message since 2026-10-20 records a `cargo xtask vm` run (grep for "xtask vm", "vm test", "Hyper-V"). Changed fact since D3's amendment: CLAUDE.md's rule of 2026-11-19 (8097dfce) makes `cargo xtask e2e` the one local path, and GitHub runs the suite. Cost each new binary pays: a line in `deploy.rs`'s build list and its staging (T10). Options: keep it, as section 14 says, for contributors; or retire it and amend D3 and section 14. Speech: none.

## 5. Cost of one behaviour here, and what would cut it

Adding a tool crate (0ff2864e, `verbatim-event-record`, verified by `git show --stat`): its crate; a class in `xtask/src/tiers.rs`; `.github/workflows/ci.yml`'s build line; `xtask/src/vm/deploy.rs` (16 lines); `crates/verbatim-e2e/src/scenario.rs`'s build list; registry and `tests/e2e.rs` for its scenario; its crate guide, the crate index, `SUMMARY.md`, `test-coverage.md`; `Cargo.lock` and the workspace-hack. A platform-neutral or logic crate also touches `LINUX_CRATES`, the pinned `12` in `coverage.rs`, and CLAUDE.md's prose list. A crate on a reducer's path adds a ninth `clippy.toml` copy. A disallowed method touches eight identical `clippy.toml` files and `DETERMINISM_ENTRIES`. A test that reads fixtures in a covered crate touches two exception lists (c5c0181a, b43b3c2d). A scenario touches `registry.rs` and `tests/e2e.rs` (two lists, held by a text-reading test) and, silently, `perf.rs`'s measure list for a measurement (T3). Moving a command onto shared code (60bd5146) left `demo` and CI behind.

Merged proposals, each verified above: derive crate lists from `TIERS` and `cargo metadata` (T10); one canonical `clippy.toml` with equality as the only check (T6); one exception list (T7); the measure set from the registry (T3); the trial list in `tests/e2e.rs` built from `registry::SCENARIOS` rather than written again (T11, merger); one binary list in `verbatim-e2e` (T10); one agent start (T12); one walker and one brace finder for every scan (T13). If ruled: the plan job without the source parse (Q2) and the VM harness's `deploy.rs` out of every new binary (Q6).

## 6. Dropped or corrected

- Reviewer B: `demo` and `nvda compare` "have no guard" against a name matching no test: contradicted by `demo.rs:103-104`, the video check, and `nvda.rs:592-593`.
- Reviewer A: the VM harness has had "no functional commit since 2026-10-07": contradicted by 0ff2864e and 8efb3da1.
- Both: `vm/scripts`' 996 lines as the VM harness's: 300 serve park, trace and the suite's crash dumps.
- Reviewer A: the pure rule "becomes the compiler's" (T22): the allow-lists cannot move; reclassified to a question.
- Both: "no step depends on another's success" in `cargo xtask ci`: determinism takes the tier step's output.
- Reviewer B: event-record 2,346 product lines (it is 2,833); NVDA tooling 3,750 lines (4,032); ratios 2.6 and 5.2 (1.5 and 4.8).
- Reviewer A: xtask's 2,129 test lines (1,953); listener lines 3509-3518 (3559-3571); the exploration commits as the deleters of every stale `GROUPS` file (adb0a7e2, b842c487, 1e6c10fb deleted three of them).
- Reviewer B: 22 shared hook entries (23); `ci.yml:403` (404); "`fn workspace()` in nine xtask modules" (five functions; eleven files derive the root from `CARGO_MANIFEST_DIR`).
- Reviewer B: removing `the_agent_comes_from_cargo_target_dir_when_set` with demo's agent code: the behaviour it tests is the one `Agent` lacks (T12).
- Reviewer A: computing the recorder's `behind` lines in `print` instead of the writer: no quoted defect or line count; dropped as unquantified.
- Reviewer A: `verbatim-synth-capture` classed in the Windows tier though it names no Windows API: true (`grep` finds no `windows::`), but no change proposed; not a finding.
- Reviewer B: time formatting written six times (about 120 lines in five crates outside this area but event-record and inspect): confirmed in part (`flight_dump.rs:40`, `artifacts.rs:297,315,326`, `record.rs:679`, `inspect/src/timestamp.rs`, `agent/src/clock.rs`), including `artifacts.rs:302,320` `unwrap_or(0)`; kept out of the table as low value and mostly outside the area.
