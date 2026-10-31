# Logic foundations: the designs of M4.5's builds

This document holds the designs of M4.5's builds (`docs/roadmap.md`, "M4.5 — Logic foundations"), one section each, each written against the code of its day. The milestone's goal, its rules, the alternatives weighed and its limits are in [logic tests](logic-tests.md), which each section implements and does not restate; where the code contradicts that design, the section says so.

## The crate split (the first build)

Status: proposed on 2026-10-31, before its adversarial review.

The build splits `crates/verbatim-outpost`, which holds three programs (the outpost, the focus listener and Core's supervisor), into crates by role, makes the listener an executable of its own, and replaces the platform-neutral dependency check with a check of the crate tiers ([logic tests](logic-tests.md), "Crates by role"). It moves code and changes no behaviour: no thread, message, byte on a pipe or wake-up changes. Only the listener's image name and the log targets of the moved modules change. The reducer and the read scheduler move as they are; the program bookkeeping, the failure paths and determinism are later builds of M4.5.

### The crates

The logic tier, in the GPL tier and with no Windows dependency:

- `verbatim-outpost-logic`: the outpost's reducer, today's `crates/verbatim-outpost/src/outpost/reducer/` whole, as the crate's root. It depends on `verbatim-model` and on `verbatim-core` for `focus_rules`. Its `pub(in crate::outpost)` items become `pub`.
- `verbatim-listener-logic`: the read scheduler and the menu end's grace (`crates/verbatim-outpost/src/read_schedule.rs`), the foreground watch (`crates/verbatim-outpost/src/foreground.rs`), and the pure vocabulary of `crates/verbatim-outpost/src/focus_read.rs` the scheduler is written in: `FocusEvent`, `Judged`, `Meta`, `Arrival`, `arrival_of`, `gap_answer` with its `VERDICTS` and `GAVE_UP` lists, every drop reason (`NOT_IN_TIME`, `MOVED_ON`, `WAITED_TOO_LONG`, `REPLACED`, `NO_WINDOW`, `NOT_FOCUSED`, `MSAA_CLASS`, the nearest window's three and the private ones the lists name), `nearest_timeout_reason` and `ABANDONED_LIMIT`. It depends on `verbatim-model` and `tracing`, which the scheduler logs through today.
- `verbatim-reader-rules`: what both readers apply before reading. NVDA's class verdict and its tables from `crates/verbatim-outpost/src/arbitration.rs` (`ClassVerdict`, `class_verdict`, `WindowClasses` with `new`, and the good, bad and shell class lists); the class name normalization from `crates/verbatim-ia2/src/class.rs` (`normalize_class_name` and NVDA's class map); the acceptance test from `crates/verbatim-outpost/src/outpost/acceptance.rs` (`EventName`, `RequestedEvents`, `WindowQueries`, `accepts_event`, `foreground_relation`); `CONSOLE_WINDOW_CLASS` from `outpost/text_reads.rs`; and `MOVED_ON_GRACE` from `outpost/window.rs`. It depends on `verbatim-core` for `ForegroundRelation`.

The supervisor's pure decisions go to a new `supervisor` module of `verbatim-core`, a platform-neutral crate: `crates/verbatim-outpost/src/supervisor/policy.rs` whole, `supervisor/generation.rs` whole, `EndReason`, and the data of `writer::Received`.

The Windows tier:

- `verbatim-outpost`, a role: the outpost process, library and `verbatim-outpost.exe`.
- `verbatim-listener`, a role, new: the focus listener, library and `verbatim-listener.exe`.
- `verbatim-supervisor`, a role, new: Core's side of starting, watching and ending children, a library `verbatim-app` uses.
- `verbatim-win32`, shared, new: the local window and process queries more than one role makes.
- `verbatim-process`, shared, existing: gains the message layer, the launch arguments, the alarm, the held processes and the children's common start and end.
- `verbatim-ia2` and `verbatim-uia`, shared, existing: `verbatim-ia2` gains the `WinEvent` thread and `fact_of_win_event`, and gives its general window wrappers to `verbatim-win32`; `verbatim-uia` gives `provider.rs` to `verbatim-win32` and re-exports it under the same names.

### Where each module goes

Each module of `crates/verbatim-outpost/src`:

- `lib.rs`: each new crate has its own root. The re-exports of `provider_process` and `console_host_of` for Core come from `verbatim-win32`.
- `main.rs`: the outpost and attach modes stay in `verbatim-outpost`; the listener mode becomes `verbatim-listener`'s `main`. `end_without_detach`, `init_tracing` and `report_error!`, which both use, go to a `child` module of `verbatim-process`.
- `protocol.rs` and `protocol/raw.rs`: to a `message` module of `verbatim-process`, the layer over its transport every end shares. `fact_of_win_event` goes to `verbatim-ia2`, beside the `WinEventKind` it reads.
- `arbitration.rs`: the class verdict and tables to `verbatim-reader-rules`; `window_class_name` and `WindowClasses::of`, which becomes the function `window_classes`, to `verbatim-win32`; the `Clock` and `ObservationClock` aliases to `verbatim-process`'s `clock` module; the rest (`Arbitrator`, `Verdict`, `decide`, `uses_uia`, `known_to_use_uia`, `post_probe_check`, `EventWindows`, `Judgement`, `time_limit`) stays in `verbatim-outpost`, the only role that probes.
- `dialog_text.rs`, `gates.rs`, `terminal.rs` with `terminal/`, `text/`: stay in `verbatim-outpost`.
- `event_thread.rs`: to `verbatim-ia2`. It installs `verbatim-ia2`'s hooks, and `verbatim-win32` sits below `verbatim-ia2`, so this is the case where the thread must sit beside the hooks.
- `focus_read.rs`: to `verbatim-listener`, but its pure vocabulary, listed above, to `verbatim-listener-logic`. `READ_DEADLINE` stays, since it adds `verbatim_uia::TRANSACTION_TIMEOUT`.
- `foreground.rs`, `read_schedule.rs`: to `verbatim-listener-logic`.
- `integrity.rs`: to `verbatim-win32`; the listener and Core read it.
- `listener.rs`: to `verbatim-listener`, but `HOOKS_IN_ARG`, `parse_hooks_in` and `format_hooks_in` to the launch arguments in `verbatim-process`.
- `outpost/`: stays in `verbatim-outpost`, except `reducer/` to `verbatim-outpost-logic`; `alarm.rs` to `verbatim-process`; `acceptance.rs` to `verbatim-reader-rules`, but `SystemWindows` to `verbatim-win32` and `EventName::of_win_event` to a function in the outpost's worker, its one caller, since `WinEventKind` is `verbatim-ia2`'s; `OutpostOptions` from `mod.rs` to the launch arguments; the reader aliases `ForegroundReader` and `WindowReadabilityReader` with `window_unreadable` to `verbatim-win32`, since both readers take them; and from `window.rs` the queries another role makes to `verbatim-win32` (`top_level_of`, `window_belongs_to_hidden_frame`, `window_is_hung`, `front_is_another_thread_of_its_application`, `foreground_window_handle`, `focus_window`, `window_owner`, `window_exists`, `event_window_unreadable`), the rest staying.
- `supervisor/`: to `verbatim-supervisor`, except `policy.rs` and `generation.rs` to `verbatim-core`; `ignored.rs` and `process.rs`'s `Target` to `verbatim-process`, which the listener, the outposts and the supervisor all hold processes through; and `IGNORE_PIDS_ARG` to the launch arguments.
- `tests/`: `target_exit.rs` stays, since it runs the real outpost binary; the other six drive a `Supervisor` against stand-ins of themselves and go to `verbatim-supervisor`, with the `lifecycle-for-tests` feature.

From the other crates: `crates/verbatim-ia2/src/window.rs`'s general functions (`exists`, `is_visible`, `is_enabled`, `is_child`, `desktop`, `class_name`, `thread_of`, `style`, `rect`, `parent`, `top_child`, `related`, `is_multiline_edit`) go to `verbatim-win32`, one definition where the outpost's `window.rs` has its own copy; the list view and tree view messages stay. `normalized_class_of` stays in `verbatim-ia2` as one line over both crates. `crates/verbatim-uia/src/provider.rs` goes to `verbatim-win32`.

### Every dependency edge

The workspace crates each depends on, after the build:

- `verbatim-outpost-logic`: `verbatim-model`, `verbatim-core`.
- `verbatim-listener-logic`: `verbatim-model`.
- `verbatim-reader-rules`: `verbatim-core`.
- `verbatim-win32`: `verbatim-model`, `verbatim-reader-rules`.
- `verbatim-process`: `verbatim-model`.
- `verbatim-ia2`: `verbatim-model`, `verbatim-text`, `verbatim-win32`, `verbatim-reader-rules`, `verbatim-process`.
- `verbatim-uia`: `verbatim-model`, `verbatim-text`, `verbatim-win32`.
- `verbatim-uia-rops`: unchanged, `verbatim-model` and `verbatim-uia`.
- `verbatim-outpost`: `verbatim-outpost-logic`, `verbatim-reader-rules`, `verbatim-win32`, `verbatim-process`, `verbatim-ia2`, `verbatim-uia`, `verbatim-uia-rops`, `verbatim-core`, `verbatim-model`, `verbatim-text`.
- `verbatim-listener`: `verbatim-listener-logic`, `verbatim-reader-rules`, `verbatim-win32`, `verbatim-process`, `verbatim-ia2`, `verbatim-uia`, `verbatim-uia-rops`, `verbatim-model`.
- `verbatim-supervisor`: `verbatim-core`, `verbatim-win32`, `verbatim-process`, `verbatim-model`.
- `verbatim-app`: `verbatim-supervisor` in place of `verbatim-outpost`, `verbatim-win32` for `provider_process`, `console_host_of` and `own_integrity`, and `verbatim-process` as today, now also for `now_us`.
- `mockapp`, a test tool: `verbatim-outpost` and `verbatim-listener`, whose readers its tests drive.

So no edge joins `verbatim-outpost`, `verbatim-listener` and `verbatim-supervisor`, or the two logic crates of different roles; no logic crate reaches a Windows crate or the Windows bindings; and no platform-neutral crate gains an edge, `verbatim-core`'s new module using only the model and itself.

### The two-way dependencies, and how each is broken

- The listener and the outpost. `focus_read.rs` uses `outpost::acceptance`, `outpost::alarm`, `outpost::window` and `outpost::text_reads::CONSOLE_WINDOW_CLASS`, while the outpost's `acceptance.rs` and `bound.rs` name `focus_read` only in comments. Broken by moving the acceptance test and the console class to `verbatim-reader-rules`, the alarm to `verbatim-process` and the shared window queries to `verbatim-win32`.
- The listener and the supervisor. `listener.rs` uses `supervisor::IgnoredProcesses`; `supervisor/owner.rs` and `supervisor/process.rs` use `listener::format_hooks_in` and `HOOKS_IN_ARG`. Broken by moving both to `verbatim-process`.
- The outpost and the supervisor. `outpost/mod.rs` uses `supervisor::Target`, `IgnoredProcesses` and `IGNORE_PIDS_ENV`; `supervisor/owner.rs` uses `outpost::window::window_exists`, and `supervisor/process.rs` `crate::OutpostOptions`. Broken by moving `Target`, the ignored processes and `OutpostOptions` to `verbatim-process`, and `window_exists` to `verbatim-win32`.
- The arbitration and the outpost. `arbitration.rs` re-exports the reducer's `Verdicts` and uses `outpost::window`; the outpost uses the `Arbitrator`. Broken by the split of `arbitration.rs` above: its outpost part stays in the outpost's crate and depends on `verbatim-outpost-logic` one way.
- The read scheduler and the focus reads. `read_schedule.rs` uses `focus_read`'s types and constants, and `focus_read.rs` runs the scheduler. Broken by moving those types and constants into `verbatim-listener-logic` beside the scheduler.
- The protocol. Every role uses it, and `fact_of_win_event` in it uses `verbatim-ia2`. Broken by the `message` module of `verbatim-process` and `fact_of_win_event` in `verbatim-ia2`.

### What is pure today, and the trait for window queries

Pure today, and moved whole: the reducer, which the ownership scan (`xtask/src/ownership.rs`) holds to the pure rule; the read scheduler and the grace, held by the same scan; the foreground watch, which uses only its own types; the class verdict, a function of class names; and the supervisor's policy and generation, apart from two clock reads. `HeldFacts::hold` reads `Instant::now()`, so it takes `now` from its caller, the owner, at the same point on the same thread; `Received`'s `now`, `elapsed_us` and `stamp` read clocks, so `Received` moves as a plain value and those three become functions in `verbatim-supervisor`.

The acceptance test's window queries come through the trait `WindowQueries`, which exists today in `outpost/acceptance.rs`: the window's class, process, parent, root and root owner, `IsChild`, the topmost style, the desktop window, the active window and the foreground window. It becomes public in `verbatim-reader-rules`; `SystemWindows` in `verbatim-win32` implements it with local calls, and the tests' fake desk moves with the decision.

Left in the Windows crates although parts are pure: the `Arbitrator`'s keeping of verdicts, which holds a probe closure and `verbatim-ia2`'s `WindowCache`; the listener's checks before a read, until the rework's third build makes them pure functions; the transport's merges; and the supervisor's owner.

### The seal: where the code contradicts the logic-tests design

The logic-tests design puts the generation's typestates in `verbatim-core` "with the `Sealed` token that only they can make". The code cannot hold that: `Sealed` is a field of `OutpostMessage`'s notices, and `OutpostMessage::Frame` carries `verbatim_process::transport::OwnedFrame`, a Windows crate's type, so `OutpostMessage` cannot live in `verbatim-core`; and the owner, not the typestates, makes four of the sealed notices (`SpareReady`, `ListenerReady`, `IdleQueried` and `NotWatched`, in `supervisor/owner.rs`).

The design here: the typestates in `verbatim-core` return a plain `Lifecycle` notice in place of today's `Notice::App(OutpostMessage)`, one variant for each message they make today (`Started`, `Ended`, `Regenerated`, `NotWatched`), with the same fields and in the same order. `Sealed` and `OutpostMessage` stay in `verbatim-supervisor`, where only the owner makes a seal, as it turns each `Lifecycle` into its message. The guarantee is kept: only the supervisor's crate makes a notice the app takes as the supervisor's, and the app's tests use `Sealed::for_tests` as today. A `Lifecycle` made elsewhere is data the app never receives.

### Two executables

Today `Supervisor::new` resolves `verbatim-outpost.exe` beside `std::env::current_exe()` (`supervisor/mod.rs`), and `Supervisor::with_executable` takes one path for tests. The owner launches every role from that one path (`launch_child`, `process::launch`), each on a launch helper thread named `verbatim-launch`: an outpost with `--pipe-in <handle> --pipe-out <handle> --target-pid <pid> [--classic-uia]`, a spare with `--spare <launch>` in place of the pid, and the listener with `--listener --pipe-in <handle> --pipe-out <handle> [--ignore-pids <pids>] [--hooks-in <pairs>]`. There is no installer: every runner stages the executables beside `verbatim.exe`.

After the build:

- `verbatim-supervisor` has `Executables`, the outpost's path and the listener's. `Supervisor::new` resolves both beside the current executable; `Supervisor::with_executables` takes both, and the stand-in tests pass their own binary as both. The owner's `Setup` holds the `Executables`, and `launch` passes the role's path to the helper. A missing `verbatim-listener.exe` fails the listener's launch, as a missing `verbatim-outpost.exe` fails an outpost's today.
- The arguments are built and parsed by one definition in `verbatim-process`: an outpost's arguments and the listener's, each with its command line and its parse, tested by a round trip. The supervisor builds them; each executable's `main` parses its own. The listener keeps `--listener` first, so the stand-in tests, which tell the roles apart by it, are unchanged, and either binary given the other's arguments refuses them at parse. `verbatim-outpost.exe` no longer accepts `--listener`.
- In development both executables are built into `target\debug`, or `target\release`, beside `verbatim.exe`. `cargo build -p verbatim-app` builds neither child, as today, so every build list adds `-p verbatim-listener`.

The start, at Verbatim's launch. The app calls `Supervisor::new`, which resolves the two paths and starts the owner thread as today. The owner calls `start_listener(false)`, which records the listener as launching and calls `launch` with `Role::Listener`. The helper thread runs `launch_child` with the listener's path; `process::launch` builds the listener's arguments, creates the two overlapped pipes and starts the process suspended in its kill-on-close job with the memory cap, then resumes it. In the child, `verbatim-listener.exe`'s `main` names its trace ids, installs tracing, parses its arguments, holds the ignored processes, takes the inherited pipes and calls `run_listener`, the same function as today, which starts the same threads. The helper sends `Launched` to the owner, which adds the pipe to the relay; the listener's `Ready` reaches the relay, and the owner sends `ListenerReady`. Once Core's own outpost is ready, the owner launches the two spares the same way with the outpost's path and `--spare`. Every step but the path and the argument builder is today's code on today's thread.

A restart. An outpost's pipe ends; the relay reads the end, and the owner calls `on_pipe_closed`, which calls the generation's `failed`, now in `verbatim-core`. Its notices come back in today's order; the owner turns each `Lifecycle` into its sealed message, `Ended` first, gives the next spare (`Assign`) or calls `launch` with `Role::Outpost`, and the helper launches from the outpost's path. When the listener's pipe ends, the owner calls `start_listener(true)`, which launches from the listener's path, and `ListenerReady` says it is a replacement. Nothing else differs.

A retirement. At its sweep the owner asks `retirement_decision`, now in `verbatim-core`, and an idle outpost is retired by `retire.rs`'s code, unchanged: the shutdown message, the wait of `SHUTDOWN_LIMIT`, then the job's kill if it has not exited, and `Ended` with `Retired`. At Verbatim's exit both roles are shut down the same way, and each child ends with `end_without_detach`, now in `verbatim-process`, as today.

So no thread is added or removed in any process, the messages are the same bytes from the same code, and nothing wakes that did not. The owner gains one `match` per lifecycle notice, on the thread that already handles it.

What names the executables, and changes with them:

- `crates/verbatim-e2e/src/scenario.rs`: the build list, `STAGED_BINARIES` and its tests, `VERBATIM_IMAGES` for crash dumps, and `OUTPOST_IMAGE`, which `outposts_shut_down_cleanly` and `outpost_crashes` compare each exit with and which today covers the listener too, so both take both images. `DEFAULT_RUST_LOG` adds `verbatim_listener=debug`, `verbatim_listener_logic=debug` and `verbatim_supervisor=debug`, and any other crate that receives a module logging at debug, so the logs keep their lines.
- `crates/verbatim-agent`: nothing; it reports any exiting process's image by name.
- `xtask/src/vm/deploy.rs`: the build list, `BuiltArtifacts`, the copied artifacts and the `Stop-Process` names; `xtask/src/vm/mod.rs`'s help; `xtask/src/perf.rs`'s `STAGED` and its tests; `xtask/src/ownership.rs`'s two paths and the reducer's allow-list, whose `crate::outpost::reducer` becomes `crate`. The golden image holds no Verbatim binary, since `cargo xtask vm deploy` copies them, so it is not rebuilt.
- `crates/verbatim-trace-waits/src/main.rs`: its default processes add `verbatim-listener.exe`, or a trace would leave the listener out.
- `.cargo/config.toml`'s `verbatim-build` alias and `.github/workflows/ci.yml`'s build step.
- `vm/scripts/Enable-VerbatimCrashDumps.ps1`: `verbatim-listener.exe` with the outposts' full dump flags. It needs elevation, so Dickson runs it once on this machine.

### The tier check in `cargo xtask ci`

Today `check_platform_neutral_deps` in `xtask/src/main.rs` runs `cargo tree -e normal` for each of the eight platform-neutral crates and fails on `windows` or `windows-core` anywhere, or `windows-sys` directly under a workspace crate. Its holes: build dependencies are not read, `winapi` is not named, and a Windows-only workspace crate with no `windows` dependency of its own, such as `verbatim-gui`, passes.

It is replaced by a module of xtask, `xtask/src/tiers.rs` (planned), the first step of `ci` as today, which reads `cargo metadata` once with no target filter, over normal and build edges, dev edges left out, so an edge gated on `cfg(windows)` is read on every host:

- Every workspace member is classed in one list: platform-neutral, logic, Windows, or tool. A member in no class fails, so a new crate is classed when it is added. Reason: the hole was a crate the check did not know.
- A platform-neutral crate reaches only platform-neutral workspace crates. Reason: it may depend on no GPL-tier crate, logic or Windows.
- A logic crate reaches only platform-neutral and logic workspace crates.
- Neither reaches `windows` or `windows-core` by any path, nor `windows-sys` or `winapi` from a workspace crate other than `workspace-hack`. Reason: third-party crates with target-gated bindings are cross-platform code, as today.
- A role's crates (`verbatim-outpost` and `verbatim-outpost-logic`; `verbatim-listener` and `verbatim-listener-logic`; `verbatim-supervisor`) reach no other role's. Reason: the rule of one-way dependencies, which the graph can hold.

Its unit tests are one per rule, over a small graph written in the test; the error names every offending path, as today's does.

### Provenance of the code moved into `verbatim-core`

`verbatim-core` is platform-neutral, so the moved comments cite the parity documents, never NVDA's source. Two comments describe NVDA's behaviour, both in `crates/verbatim-outpost/src/supervisor/policy.rs`:

- `HeldFacts`'s comment, "NVDA's limiter rule", becomes a citation of `docs/parity/backends.md`, "MSAA winevent flood control".
- `may_start_outpost`'s comment, "as NVDA drops a background application's notification", becomes a citation of `docs/parity/focus-events.md`, "Background events".

`generation.rs` and the moved parts of `supervisor/mod.rs` name nothing of NVDA's. The review checks every moved comment again, and the comment of `crates/verbatim-core/src/focus_rules.rs` that says the ported rules are defined in `verbatim-outpost` names `verbatim-reader-rules`.

### Documents and the workspace-hack crate

- `docs/crates/`: `verbatim-outpost.md` split into guides for `verbatim-outpost`, `verbatim-listener` and `verbatim-supervisor`; short guides for `verbatim-outpost-logic`, `verbatim-listener-logic`, `verbatim-reader-rules` and `verbatim-win32`; and `verbatim-process.md`, `verbatim-ia2.md`, `verbatim-uia.md` and `verbatim-core.md` for what each gains or gives, with `docs/crates/readme.md`'s order.
- `CLAUDE.md`: "NVDA provenance" names three kinds of crate, the platform-neutral, the logic crates (GPL tier, may hold ported behaviour, no Windows dependency) and the Windows-specific; the standing rule on ported code says the GPL-tier crates; and "Commands" describes the tier check.
- `docs/architecture.md`: the process list adds the focus listener's process, decision D14's "Where the rules run" names `verbatim-reader-rules` with an amended-on line, and the crate list changes.
- `docs/tooling/`: `e2e.md`, `inspect.md`, `measuring.md`, `troubleshooting.md` and `vm.md` for the build lists, the staged files and the images; `setup.md` for the check.
- Every other document that names a moved path: `cargo xtask docs` finds the backticked paths, and a search for `verbatim-outpost` the rest, 47 Markdown files outside `experiments/` today.
- The workspace-hack crate: `cargo hakari generate` and `cargo hakari manage-deps` after the new crates exist, since each needs its `workspace-hack` line and `verbatim-process` gains `serde`, `serde_json` and `simdutf8`; `cargo xtask ci` checks both.

### Measurements before and after

Every figure names its mode. "Before" is the commit before the build's first, from a clean worktree named with `--verbatim-exe`; "after" is the build's last code commit.

- Wake-ups and processor time: `cargo xtask trace scenario measure_keys_text_box --after 15 --seconds 8` and `cargo xtask trace scenario conhost_flood --after 8 --seconds 6`, debug build, `VERBATIM_E2E_RUST_LOG=info`, and idle Verbatim by hand, as `docs/tooling/measuring.md` gives them. Compared by process: before, the listener is the `verbatim-outpost.exe` whose pid verbatim.exe's `launched` line with `role=Listener` names. Expected equal within the spread of two runs of one build.
- Each child's memory: idle Verbatim started directly, read 12 seconds in, with Core's own outpost and both spares ready: each child's private bytes and working set (`Get-Process`, with the command line from `Win32_Process` to find the listener before), and their total, in a debug and a release build, with each executable's size. Expected: private bytes unchanged; the total working set may grow by the listener's code pages, no longer shared with the outposts'.
- The listener's start: from `launched` to "focus listener ready" in verbatim.exe's log, over the full suite's runs.
- `cargo xtask ci`'s time: wall time on this machine, twice each before and after: a rerun with nothing changed, and a run after a one-line change to `crates/verbatim-model/src/lib.rs`, which rebuilds every dependent.

### End-to-end runs and the order of work

`outpost_crash_recovery` and `return_after_retirement`, which start, restart and retire outposts, run once during the build to check the launch path, and three times at its end; the full suite runs once at the end, since how the listener starts changes, leaving out the scenarios `docs/flaky-tests.md` records. Its time is the sum of the last full run's `finished in` lines. A listener's restart has no scenario; the supervisor's stand-in tests cover it.

The build is four commits:

- Move the shared pieces out of `verbatim-outpost`: the message layer, launch arguments, alarm, clocks, held processes and the children's start and end to `verbatim-process`; `verbatim-reader-rules`; `verbatim-win32`, with `provider.rs` and `verbatim-ia2`'s window functions; the `WinEvent` thread and `fact_of_win_event` to `verbatim-ia2`. `verbatim-outpost` still holds the three programs, its cycles gone.
- Split by role: `verbatim-outpost-logic`, `verbatim-listener-logic`, `verbatim-listener` with its executable, `verbatim-supervisor`, `verbatim-core`'s `supervisor` module, and every runner and script that names an executable, in one commit, since the executable and its staging change together.
- The tier check in `cargo xtask ci`.
- The documents.

The measurements before are taken first; `cargo xtask ci` runs at the end, then the end-to-end runs and the measurements after.

### Limitations

- Splitting the image costs the listener its sharing of code pages with the outposts, measured above.
- Logic that is pure in parts stays in the Windows crates, as listed above, outside the Linux build and the generated tests until moved.
- The reducer keeps its own `MOVED_ON_GRACE` beside `verbatim-reader-rules`'s, and `provider.rs` keeps its own console class; joining each is a follow-up.

### Questions for Dickson

- The seal. The typestates cannot make the `Sealed` token, as the section above shows. Recommended: they return a plain `Lifecycle` notice and `Sealed` stays in `verbatim-supervisor`, made only by the owner, which keeps the guarantee the seal gives. The other choice, an `OutpostMessage` without its frame in `verbatim-core` with a public way to seal, lets any crate seal and so ends the guarantee.
- `provider.rs` to `verbatim-win32`, beyond the design's list. Recommended: move it, with `verbatim-uia` re-exporting it, so the window queries have one crate under both backends. Otherwise `verbatim-win32` depends on `verbatim-uia`, and `verbatim-ia2` on the UI Automation client through it.
- The executables' layout. Recommended: both beside `verbatim.exe`, found as the outpost and the synthesizer host are found today, rather than in a folder of their own.
- `verbatim-ext-api`'s tier. The logic-tests design names it among the crates that must not depend on a logic crate, but it is not on the platform-neutral list. Recommended: class it platform-neutral, since it has no dependency yet and an extension's SDK should not carry the GPL tier.
- The `Arbitrator`'s pure decisions. Recommended: leave them in `verbatim-outpost` in this build, as the design names only the reducer for `verbatim-outpost-logic`, and decide in M4.6's design whether its tests need them.
- The crash dump script needs your elevated run once after the build, for `verbatim-listener.exe`'s dumps.

## xtask on Linux and the Linux job (a build of its own)

M4.5's build that lets `xtask` build on any platform and adds a Linux job to CI, as `docs/design/logic-tests.md` ("Tools and determinism") asks. Written against the code of 2026-10-31 (commit 62c340d1).

### What is Windows-only in xtask today

`xtask` builds only on Windows: `xtask/Cargo.toml` depends unconditionally on `windows` and on workspace crates that depend on it (`verbatim-agent`, `verbatim-control`, `verbatim-e2e`, `verbatim-uia`, `verbatim-uia-rops`), and `xtask/src/main.rs` imports `windows` at the top for `warm-speech`.

By module, from what each one uses:

- Windows-only: `conformance` (windows, `verbatim-uia`, `verbatim-uia-rops`, `verbatim-term-sim`, `verbatim-e2e`), `record` (windows, `verbatim-term-sim`, serde), `nvda` (`verbatim-agent`, `verbatim-control`, `verbatim-model`), `demo` and `vm` (`verbatim-e2e`, `verbatim-config`, `verbatim-agent`, base64, PowerShell and Hyper-V), `park` and `trace` (Windows tools: tscon, schtasks, WPR), `perf` (uses `trace`), and in `main.rs` the functions `warm_speech`, `activate_speech` and `windows_terminal`.
- Portable, using the standard library alone: `docs`, `ownership`, `waits` and `shapes`, and in `main.rs` the dependency check `check_platform_neutral_deps` and `ci` itself.

#### Gating rules

- A whole module is gated, never single functions inside it: `#[cfg(windows)] mod conformance;` and so on for each Windows-only module, and `#[cfg(windows)]` on the three Windows functions in `main.rs` and on every import only they use (the `windows` imports and `std::time::Instant`, which only `warm_speech` uses), since an unused import fails Clippy with warnings denied. Reason: one attribute per module is easy to audit, and a Windows module never has to keep a Linux branch compiling.
- Every dependency except `workspace-hack` moves to `[target.'cfg(windows)'.dependencies]` in `xtask/Cargo.toml`, including the portable ones (base64, serde, serde_json, `verbatim-model`, `verbatim-config`, `verbatim-term-sim`), since only Windows-only modules use them. Reason: a dependency lives under the same condition as the code that uses it, so the Linux build compiles nothing it does not run. `workspace-hack` stays unconditional because `cargo hakari manage-deps` requires every package to depend on it.
- The Windows-only commands' match arms in `main` and their lines in the usage text carry `#[cfg(windows)]`, so on Linux the usage lists only `ci` and `docs` and any other command gets the usage and exit code 2. Reason: one dispatch path, with no "not supported here" branch to test.
- Hakari's generated output does not change: its `platforms` in `.config/hakari.toml` are the two Windows triples, on which xtask's dependencies are unchanged. The builder confirms it with `cargo hakari generate --diff`.

#### What the checks assume about Windows

- The documentation check (`xtask/src/docs.rs`) is already portable: it builds paths with `/`, compares every component's letter case against directory listings (so a link that differs only in case fails on both systems), and reads lines on `\n` with trailing whitespace trimmed, so both the CRLF checkout on GitHub's Windows runner and Linux's LF checkout read the same. The repository stores every document and fixture with LF (`git ls-files --eol`).
- The scans (`waits.rs`, `shapes.rs`, `ownership.rs`) are portable: paths are normalised with `replace('\\', "/")` or compared with `Path::ends_with`, files are read with `str::lines`, which drops a trailing `\r`, and Windows names such as `Start-Sleep` or `verbatim_input_windows` are only text they search for.
- The platform-neutral dependency check is the one host-dependent check: `cargo tree` reads the graph for the host's target, so on Linux it would not see a dependency gated on `cfg(windows)` and would pass where Windows fails. Rule: it passes `--target all`, so it reads the same graph on every host. Reason: the check's answer must not depend on where it ran. On today's graph, `cargo tree --target all` for every platform-neutral crate reaches `windows-sys` only (beneath third-party crates and `workspace-hack`, which the check allows) and neither `windows` nor `windows-core`. `cargo tree` reads the graph and builds nothing, so this is not a build naming a target. Its workspace-crate test compares paths with `std::path::MAIN_SEPARATOR`, which is correct on both systems.

### `cargo xtask ci` on Linux

`ci` keeps one entry point, and what it runs depends on the host:

- On every host: the dependency check, the three scans and the documentation check.
- On Windows, as today: the hakari checks, rustfmt, and Clippy and unit tests for the whole workspace.
- On any other host: Clippy (warnings denied, all targets) and unit tests for the crates in a new constant, `LINUX_CRATES`, in `xtask/src/main.rs` beside `PLATFORM_NEUTRAL_CRATES`, each passed as `-p`.

Rules, each with its reason:

- The crate list lives in xtask, not in the workflow. Reason: `ci` is the standard check so that local runs and GitHub cannot drift, as its own doc comment says.
- rustfmt and the hakari checks run only on Windows. Reason: they read source and metadata the same way on every host, and the Windows jobs already run them.
- `LINUX_CRATES` before the split: `verbatim-core` and `xtask`. Core's tests build `verbatim-model`, `verbatim-text` and `verbatim-term-sim` as its dependencies; `xtask` is listed so the unit tests of the documentation check and the scans run on Linux. After the split: add `verbatim-outpost-logic`, `verbatim-listener-logic` and `verbatim-reader-rules`.
- No step names a target, so Linux builds for the runner's own architecture, as Windows does.

### What builds on Linux today

Checked on this machine with `cargo check --target x86_64-unknown-linux-gnu --all-targets` into a scratch target directory (a check builds no binaries and runs no tests):

- Checks cleanly: `verbatim-model`, `verbatim-term-sim`, `verbatim-config`, `verbatim-input` and `verbatim-audio`. `workspace-hack` builds as part of each: its unconditional dependencies are portable, and its `windows-sys` and `getrandom` entries sit under Windows-target tables that Linux skips.
- Could not be checked from Windows: `verbatim-text`, and so `verbatim-core`, `verbatim-speech` and `verbatim-i18n`, which depend on it. `jieba-rs` (`crates/verbatim-text/Cargo.toml`) embeds its dictionary through `include-flate`, whose default `zstd` feature builds `zstd-sys`, a C library, for the target. From Windows that needs a Linux C compiler and C library headers, and this machine has neither (Visual Studio's clang finds no `stdlib.h` for Linux). GitHub's `ubuntu-latest` runner has gcc, so this is an obstacle only to checking locally, not to the job.
- No Windows code in the crates the job builds: a search of `verbatim-core`, `verbatim-model`, `verbatim-text` and `verbatim-term-sim` finds no `cfg(windows)`, `target_os` or `std::os::windows`. Fixture paths are joined with `/` from `CARGO_MANIFEST_DIR` (`crates/verbatim-core/tests/replay_fixture.rs`, `crates/verbatim-term-sim/src/conformance.rs`). Fixtures are JSON, where line endings outside strings do not matter, and the flight recorder's dump reader trims `\r` (`crates/verbatim-core/src/dump.rs`).

So no obstacle is known in today's code. Whether `verbatim-text`'s and `verbatim-core`'s tests pass on Linux is proven only by the job's first run, and the builder fixes whatever that run finds within this build.

### The Linux job

- Job `ci-linux` in `.github/workflows/ci.yml`, on `ubuntu-latest`, with the same `needs: changes` and condition as `ci` and `ci-arm64` (a code change, not a run started by hand), so a documentation-only change does not run it.
- Steps: checkout (no submodules: eSpeak NG is not in the Linux crates' graph), `Swatinem/rust-cache@v2`, then `cargo xtask ci`. The toolchain comes from `rust-toolchain.toml` (1.99.0 with rustfmt and Clippy) on the first cargo command, as on Windows. No wxWidgets cache, cargo-hakari or speech warm-up, since none of them runs on Linux.
- The documentation-only path: the `docs` job moves from `windows-latest` to `ubuntu-latest`. Reason: it then builds only xtask's portable part and `workspace-hack` rather than the `windows` crate and the end-to-end crates, so a documentation change gets its answer sooner.
- Expected time: on this machine (12 threads, debug, cold), building the tests of core and its dependencies took 31 s and running them 66 s. `ubuntu-latest` for a public repository has 4 processors, so a cold run is estimated at 4 to 6 minutes for Clippy, the build and the tests, and less with the cache; the Windows `ci` job takes 6.5 to 8.5 minutes, so running beside it adds no wall time. The first runs give the real figure, and the design records it then. The Linux cache adds roughly a gigabyte to the repository's 10 GB Actions cache.

#### Before and after the split

- Before the split: xtask's gating, `--target all` in the dependency check, `ci`'s host-dependent steps with `LINUX_CRATES` holding `verbatim-core` and `xtask`, the `ci-linux` job, and the `docs` job on Linux. None depends on the split.
- After the split: the three logic crates are added to `LINUX_CRATES`. A logic crate that reaches a Windows crate unconditionally then fails to build on Linux, which proves what the tier check reads from the graph; an edge gated on `cfg(windows)` builds on Linux, so only the tier check, reading every target, catches it.

### Verifying locally on this Windows machine

- WSL is not installed (`wsl.exe -l -v`), and Docker is not available.
- A check-only cross target works for every crate that does not reach `verbatim-text`: `rustup target add x86_64-unknown-linux-gnu` (done on this machine on 2026-10-31 for this study), then `cargo check --target x86_64-unknown-linux-gnu --target-dir <scratch> -p xtask --all-targets`, and `cargo clippy` the same way. That proves xtask's gating, which is most of this build's code. It cannot reach `verbatim-core` or the logic crates, for the `zstd-sys` reason above, so their Linux build is verified by the GitHub run, which the standing rules already have every builder read after pushing.
- The rule "no command names a target" (`CLAUDE.md`, "Coding Standards") exists so that each architecture is built and tested on its own machine. A check that produces no binary and runs no test does not weaken that, but the rule's wording admits no exception, so a cross-target check needs Dickson's agreement (a question below). Without it, the builder verifies on GitHub alone.

### Documents to change

- `CLAUDE.md`, "Commands": after the sentence on what `cargo xtask ci` runs, add: "On a Linux host it runs the dependency check, the scans and the documentation check, then Clippy and unit tests for the crates listed in `LINUX_CRATES` (`xtask/src/main.rs`); GitHub Actions runs it there as the `ci-linux` job on `ubuntu-latest`, and the documentation-only check also runs on Linux." If Dickson agrees to the check-only exception, "Coding Standards" gains: "The one exception is a check-only `cargo check` or `cargo clippy` for `x86_64-unknown-linux-gnu`, which builds and runs nothing."
- `docs/tooling/setup.md`, its paragraph on the CI jobs: the `ci-linux` job, and the Linux target for the local check if agreed.
- `xtask/src/main.rs`'s module comment: which commands exist on which hosts.

### Decisions

- A builder may run a check-only `cargo check` or `cargo clippy` with `--target x86_64-unknown-linux-gnu` on this machine, and xtask's dependency check runs `cargo tree --target all`; both are stated exceptions to "no command names a target", since neither builds or tests (Dickson, 2026-10-31).
- The Linux job builds and tests the whole platform-neutral tier (`verbatim-model`, `verbatim-text`, `verbatim-speech`, `verbatim-config`, `verbatim-i18n`, `verbatim-input`, `verbatim-audio`, `verbatim-core`) and xtask, and the logic crates once the split adds them (Dickson, 2026-10-31).
- WSL is not installed; the GitHub run is the Linux build's check (Dickson, 2026-10-31).
