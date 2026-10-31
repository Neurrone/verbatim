# Logic foundations: the designs of M4.5's builds

This document holds the designs of M4.5's builds (`docs/roadmap.md`, "M4.5 — Logic foundations"), one section each, each written against the code of its day. The milestone's goal, its rules, the alternatives weighed and its limits are in [logic tests](logic-tests.md), which each section implements and does not restate; where the code contradicts that design, the section says so.

## The crate split (the first build)

Status: proposed on 2026-10-31, revised after its adversarial review (`experiments/logic-tests-study/crate-split-design-review.md`).

The build splits `crates/verbatim-outpost`, which holds three programs (the outpost, the focus listener and Core's supervisor), into crates by role, makes the listener an executable of its own, and replaces the platform-neutral dependency check with a check of the crate tiers ([logic tests](logic-tests.md), "Crates by role"). It moves code and changes no behaviour: no thread, message, byte on a pipe or wake-up changes. Only the listener's image name and the log targets of the moved modules change. The reducer and the read scheduler move as they are; the program bookkeeping, the failure paths and determinism are later builds of M4.5.

### The crates

The logic tier, in the GPL tier and with no Windows dependency:

- `verbatim-outpost-logic`: the outpost's reducer, today's `crates/verbatim-outpost/src/outpost/reducer/` whole, as the crate's root. It depends on `verbatim-model`, on `verbatim-core` for `focus_rules`, and on `verbatim-reader-rules` for the two values it shares with the listener (below). Its `pub(in crate::outpost)` and `pub(crate)` items, such as `gap_record::BOUND_US` and `follow::Route`, become `pub`.
- `verbatim-listener-logic`: the read scheduler and the menu end's grace (`crates/verbatim-outpost/src/read_schedule.rs`), the foreground watch (`crates/verbatim-outpost/src/foreground.rs`), and the pure vocabulary of `crates/verbatim-outpost/src/focus_read.rs` the scheduler is written in: `FocusEvent`, `Judged`, `Meta`, `Arrival`, `arrival_of`, `gap_answer` with its `VERDICTS` and `GAVE_UP` lists, every drop reason (`NOT_IN_TIME`, `MOVED_ON`, `WAITED_TOO_LONG`, `REPLACED`, `NO_WINDOW`, `NOT_FOCUSED`, `MSAA_CLASS`, the nearest window's three and the private ones the lists name), `nearest_timeout_reason`, `NEAREST_FAILED_FAST_BOUND` and `ABANDONED_LIMIT`. It depends on `verbatim-model`, on `verbatim-reader-rules` for the UIA focus wait, and on `tracing`, which the scheduler logs through today.
- `verbatim-reader-rules`: what both readers apply before reading. NVDA's class verdict and its tables from `crates/verbatim-outpost/src/arbitration.rs` (`ClassVerdict`, `class_verdict`, `WindowClasses` with `new`, and the good, bad and shell class lists); the class name normalization from `crates/verbatim-ia2/src/class.rs` (`normalize_class_name` and NVDA's class map); the acceptance test from `crates/verbatim-outpost/src/outpost/acceptance.rs` (`EventName`, `RequestedEvents`, `WindowQueries`, `accepts_event`, `foreground_relation`); `CONSOLE_WINDOW_CLASS` from `outpost/text_reads.rs`; `MOVED_ON_GRACE` from `outpost/window.rs`; and a new constant for the UIA focus wait, one second. It depends on `verbatim-core` for `ForegroundRelation`.

Two facts the split would otherwise leave in two role crates are joined in `verbatim-reader-rules`, because each is one rule both readers apply and no test could tie two copies in crates that may not depend on each other:

- The moved-on grace. The reducer's own `MOVED_ON_GRACE` (`reducer/mod.rs`, 500 ms) is the same NVDA rule as `window.rs`'s, how long a call into a window is waited for before the user is taken to have moved on, so the reducer uses `verbatim-reader-rules`' constant and its copy goes.
- The UIA focus wait. The gap record's bound (`gap_record::BOUND_US`, one second), how long an outpost waits for a UIA focus, and the listener's `NEAREST_FAILED_FAST_BOUND`, under which a nearest window's lookup that timed out counts as failing fast, are equal by definition: the latter's comment says a lookup that took that long has held its application's events as long as the record waits. Both are derived from the one constant.

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
- `outpost/`: stays in `verbatim-outpost`, except `reducer/` to `verbatim-outpost-logic`; `alarm.rs` to `verbatim-process`; `acceptance.rs` to `verbatim-reader-rules`, but `SystemWindows` to `verbatim-win32` and `EventName::of_win_event` to a function in the outpost's worker, its one caller, since `WinEventKind` is `verbatim-ia2`'s; `OutpostOptions` from `mod.rs` to the launch arguments; the reader aliases `ForegroundReader` and `WindowReadabilityReader` with `window_unreadable` to `verbatim-win32`, since both readers take them; and from `window.rs` the queries another role makes to `verbatim-win32` (`top_level_of`, `window_belongs_to_hidden_frame` with the `window_is_hidden_frame`, `core_pid` and `parent_pid` it calls, `window_is_hung`, `front_is_another_thread_of_its_application`, `foreground_window_handle`, `focus_window`, `window_owner`, `window_exists`, `event_window_unreadable`, and for the supervisor's hang check `top_level_windows`, `window_is_visible` and `served_by`, which joins `provider.rs`'s function of the same name it wraps), the rest staying.
- `supervisor/`: to `verbatim-supervisor`, except `policy.rs` and `generation.rs` to `verbatim-core`; `ignored.rs` and `process.rs`'s `Target`, with the `stem_of` that both `Target` and the log naming's `image_stem` call, to `verbatim-process`, which the listener, the outposts and the supervisor all hold processes through; and `IGNORE_PIDS_ARG` to the launch arguments. `application_is_hung` stays in the supervisor and calls `verbatim-win32`.
- `tests/`: `target_exit.rs` stays, since it runs the real outpost binary; the other six drive a `Supervisor` against stand-ins of themselves and go to `verbatim-supervisor`, with the `lifecycle-for-tests` feature.

From the other crates: `crates/verbatim-ia2/src/window.rs`'s general functions (`exists`, `is_visible`, `is_enabled`, `is_child`, `desktop`, `class_name`, `thread_of`, `style`, `rect`, `parent`, `top_child`, `related`, `is_multiline_edit`) go to `verbatim-win32`, one definition where the outpost's `window.rs` has its own copy; the list view and tree view messages stay. `normalized_class_of` stays in `verbatim-ia2` as one line over both crates. `crates/verbatim-uia/src/provider.rs` goes to `verbatim-win32`, its private console class name replaced by `verbatim-reader-rules`' constant.

### Every dependency edge

The workspace crates each depends on, after the build:

- `verbatim-outpost-logic`: `verbatim-model`, `verbatim-core`, `verbatim-reader-rules`.
- `verbatim-listener-logic`: `verbatim-model`, `verbatim-reader-rules`.
- `verbatim-reader-rules`: `verbatim-core`.
- `verbatim-win32`: `verbatim-model`, `verbatim-reader-rules`; for its tests, `verbatim-test-console`, which `provider.rs`'s and `window.rs`'s console tests use.
- `verbatim-process`: `verbatim-model`.
- `verbatim-ia2`: `verbatim-model`, `verbatim-text`, `verbatim-win32`, `verbatim-reader-rules`, `verbatim-process`.
- `verbatim-uia`: `verbatim-model`, `verbatim-text`, `verbatim-win32`.
- `verbatim-uia-rops`: unchanged, `verbatim-model` and `verbatim-uia`.
- `verbatim-outpost`: `verbatim-outpost-logic`, `verbatim-reader-rules`, `verbatim-win32`, `verbatim-process`, `verbatim-ia2`, `verbatim-uia`, `verbatim-uia-rops`, `verbatim-core`, `verbatim-model`, `verbatim-text`.
- `verbatim-listener`: `verbatim-listener-logic`, `verbatim-reader-rules`, `verbatim-win32`, `verbatim-process`, `verbatim-ia2`, `verbatim-uia`, `verbatim-uia-rops`, `verbatim-model`, and `verbatim-core`, for the `SNAP_RESULTS_ACTIVITY` that `FocusReader` reads.
- `verbatim-supervisor`: `verbatim-core`, `verbatim-win32`, `verbatim-process`, `verbatim-model`.
- `verbatim-app`: `verbatim-supervisor` in place of `verbatim-outpost`, `verbatim-win32` for `provider_process`, `console_host_of` and `own_integrity`, and `verbatim-process` as today, now also for `now_us`.
- `mockapp`, a test tool: `verbatim-outpost` and `verbatim-listener`, whose readers its tests drive, `verbatim-listener-logic` for `Judged` and the drop reasons its tests compare, and `verbatim-win32` for `window_classes`, which `tests/arbitration.rs` calls.

Third-party dependencies that move with their code: `crossbeam-channel` to `verbatim-ia2` with the `WinEvent` thread; `serde`, `serde_json`, `simdutf8` and `tracing-subscriber` to `verbatim-process` with the message layer and `init_tracing`. The root `Cargo.toml` gains a `[workspace.dependencies]` line for each of the six new crates.

So no edge joins `verbatim-outpost`, `verbatim-listener` and `verbatim-supervisor`, or the two logic crates of different roles; no logic crate reaches a Windows crate or the Windows bindings; and no platform-neutral crate gains an edge, `verbatim-core`'s new module using only the model and itself.

### The two-way dependencies, and how each is broken

- The listener and the outpost. `focus_read.rs` uses `outpost::acceptance`, `outpost::alarm`, `outpost::window` and `outpost::text_reads::CONSOLE_WINDOW_CLASS`, while the outpost's `acceptance.rs` and `bound.rs` name `focus_read` only in comments. Broken by moving the acceptance test and the console class to `verbatim-reader-rules`, the alarm to `verbatim-process` and the shared window queries to `verbatim-win32`.
- The listener and the supervisor. `listener.rs` uses `supervisor::IgnoredProcesses`; `supervisor/owner.rs` and `supervisor/process.rs` use `listener::format_hooks_in` and `HOOKS_IN_ARG`. Broken by moving both to `verbatim-process`.
- The outpost and the supervisor. `outpost/mod.rs` uses `supervisor::Target`, `IgnoredProcesses` and `IGNORE_PIDS_ENV`; `supervisor/owner.rs` uses `outpost::window::window_exists`, `supervisor/process.rs`'s `application_is_hung` uses `outpost::window`'s `top_level_windows`, `window_is_visible` and `window_is_hung`, and `supervisor/process.rs` uses `crate::OutpostOptions`. Broken by moving `Target` with `stem_of`, the ignored processes and `OutpostOptions` to `verbatim-process`, and the four window queries, with `served_by` under `top_level_windows`, to `verbatim-win32`.
- The arbitration and the outpost. `arbitration.rs` re-exports the reducer's `Verdicts` and uses `outpost::window`; the outpost uses the `Arbitrator`. Broken by the split of `arbitration.rs` above: its outpost part stays in the outpost's crate and depends on `verbatim-outpost-logic` one way.
- The read scheduler and the focus reads. `read_schedule.rs` uses `focus_read`'s types and constants, and `focus_read.rs` runs the scheduler. Broken by moving those types and constants into `verbatim-listener-logic` beside the scheduler.
- The protocol. Every role uses it, and `fact_of_win_event` in it uses `verbatim-ia2`. Broken by the `message` module of `verbatim-process` and `fact_of_win_event` in `verbatim-ia2`.

### What is pure today, and the trait for window queries

Pure today, and moved whole: the reducer, which the ownership scan (`xtask/src/ownership.rs`) holds to the pure rule; the read scheduler and the grace, held by the same scan; the foreground watch, which uses only its own types; the class verdict, a function of class names; and the supervisor's policy and generation, apart from two clock reads. `HeldFacts::hold` reads `Instant::now()` only while no fact is held, so it takes a lazy clock, `now: impl FnOnce() -> Instant`, which it calls at that same point; its callers in the owner pass `Instant::now`, so the reads are today's exactly, one for an application's first held fact, and the core function reads no clock of its own. `Received`'s `now`, `elapsed_us` and `stamp` read clocks, so `Received` moves as a plain value and those three become functions in `verbatim-supervisor` with their bodies and their order of reads unchanged.

The acceptance test's window queries come through the trait `WindowQueries`, which exists today in `outpost/acceptance.rs`: the window's class, process, parent, root and root owner, `IsChild`, the topmost style, the desktop window, the active window and the foreground window. It becomes public in `verbatim-reader-rules`; `SystemWindows` in `verbatim-win32` implements it with local calls, and the tests' fake desk moves with the decision.

Items made public by the move need what `CLAUDE.md` asks of a public API. The reducer's `pub(in crate::outpost)` and `pub(crate)` items and fields, the `pub(super)` items of `policy.rs` and `generation.rs`, and the scheduler's and acceptance test's `pub(crate)` items number about 55 with no doc comment above them today, by a count of the items and fields in those files; the builder writes each one's comment, and meets Clippy's pedantic lints for exported items (`must_use_candidate`, `missing_panics_doc` and the rest), which `cargo xtask ci` denies. Intra-doc links that the split puts across crates, such as the policy's links to the supervisor's constants and `FocusEvent`'s to `FocusReader::check_fact`, are rewritten as plain text or as links into the crate that now holds the item; `cargo xtask ci` does not run `cargo doc`, so the builder runs it once.

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

- `crates/verbatim-e2e/src/scenario.rs`: the build list, `STAGED_BINARIES` and its tests, `VERBATIM_IMAGES` for crash dumps, and `OUTPOST_IMAGE`, which `outposts_shut_down_cleanly` and `outpost_crashes` compare each exit with and which today covers the listener too, so both take both images, and a crash's dump name is built from the exit's own image (`outpost_crashes`, today `format!("{OUTPOST_IMAGE}.{}.dmp", ...)`), so a listener's crash finds `verbatim-listener.exe.<pid>.dmp`. `DEFAULT_RUST_LOG` adds `verbatim_listener=debug` and `verbatim_supervisor=debug`, so the logs keep their lines; a directive matches as a prefix of the target, so these cover the logic crates too, and the shared modules moved elsewhere log nothing at debug.
- `crates/verbatim-agent`: nothing; it reports any exiting process's image by name.
- `xtask/src/vm/deploy.rs`: the build list, `BuiltArtifacts`, the copied artifacts and the `Stop-Process` names; `xtask/src/vm/mod.rs`'s help; `xtask/src/perf.rs`'s `STAGED` and its tests; `xtask/src/ownership.rs`'s two paths and both allow-lists: the reducer's `crate::outpost::reducer` becomes `crate` and it gains `verbatim_reader_rules`; the read scheduler's `crate::focus_read` and `crate::protocol` become the paths in `verbatim-listener-logic` that the scheduler's vocabulary takes, and it gains `verbatim_reader_rules`. The golden image holds no Verbatim binary, since `cargo xtask vm deploy` copies them, so it is not rebuilt.
- `crates/verbatim-trace-waits/src/main.rs`: its default processes add `verbatim-listener.exe`, or a trace would leave the listener out.
- `.cargo/config.toml`'s `verbatim-build` alias and `.github/workflows/ci.yml`'s build step.
- `vm/scripts/Enable-VerbatimCrashDumps.ps1`: `verbatim-listener.exe` with the outposts' full dump flags. It needs elevation, so Dickson runs it once on this machine.

### The tier check in `cargo xtask ci`

Today `check_platform_neutral_deps` in `xtask/src/main.rs` runs `cargo tree -e normal` for each of the eight platform-neutral crates and fails on `windows` or `windows-core` anywhere, or `windows-sys` directly under a workspace crate. Its holes: build dependencies are not read, `winapi` is not named, and a Windows-only workspace crate with no `windows` dependency of its own, such as `verbatim-gui`, passes.

It is replaced by a module of xtask, `xtask/src/tiers.rs` (planned), the first step of `ci` as today, which reads `cargo metadata` once with no target filter, over normal and build edges, dev edges left out, so an edge gated on `cfg(windows)` is read on every host:

- Every workspace member is classed in one list: platform-neutral, logic, Windows, or tool. A member in no class fails, so a new crate is classed when it is added. Reason: the hole was a crate the check did not know. `workspace-hack` is classed platform-neutral, since every crate depends on it, its `windows-sys` covered by the exception below; `xtask` is a tool.
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
- This document: this section names about forty paths that move. Each commit updates the paths it moves; the last code commit reduces the section to its built form, the problem, the choices and their reasons, pointing to the crate guides for the mechanism, as `CLAUDE.md` asks of a built design.
- The workspace-hack crate: `cargo hakari generate` and `cargo hakari manage-deps` after the new crates exist, since each needs its `workspace-hack` line and the third-party dependencies listed above move between crates; `cargo xtask ci` checks both.

### Measurements before and after

Every figure names its mode. "Before" is the commit before the build's first, run from a clean worktree of it with that worktree's own xtask, since the new harness refuses an old build (`xtask/src/perf.rs`'s `STAGED` will require `verbatim-listener.exe`); "after" is the build's last code commit.

- Wake-ups and processor time: `cargo xtask trace scenario measure_keys_text_box --after 15 --seconds 8` and `cargo xtask trace scenario conhost_flood --after 8 --seconds 6`, debug build, `VERBATIM_E2E_RUST_LOG=info`, and idle Verbatim by hand, as `docs/tooling/measuring.md` gives them. `by-name.txt` totals by image and thread name, so before the split the listener's `verbatim-event` and I/O threads merge there with the outposts' of the same names. The listener's before figure is therefore summed from `waits.txt`, whose rows name each thread's process id: the rows of the `verbatim-outpost.exe` whose pid verbatim.exe's `launched` line with `role=Listener` names. After, it is `verbatim-listener.exe`'s total. Compared: the listener's figure, the outposts' (before, the image's total less the listener's rows), and all children's total. Expected equal within the spread of two runs of one build.
- Each child's memory: idle Verbatim started directly, read 12 seconds in, with Core's own outpost and both spares ready: each child's private bytes and working set (`Get-Process`, with the command line from `Win32_Process` to find the listener before), and their total, in a debug and a release build, with each executable's size. Expected: private bytes unchanged; the total working set may grow by the listener's code pages, no longer shared with the outposts'.
- The listener's start: from `launched` to "focus listener ready" in verbatim.exe's log, over the full suite's runs.
- `cargo xtask ci`'s time: wall time on this machine, twice each before and after: a rerun with nothing changed, and a run after a one-line change to `crates/verbatim-model/src/lib.rs`, which rebuilds every dependent.

### End-to-end runs and the order of work

`outpost_crash_recovery` and `return_after_retirement`, which start, restart and retire outposts, run once during the build to check the launch path, and three times at its end; the full suite runs once at the end, since how the listener starts changes, leaving out the scenarios `docs/flaky-tests.md` records. Its time is the sum of the last full run's `finished in` lines. A listener's restart has no scenario; the supervisor's stand-in tests cover it.

The build is three commits, each passing `cargo xtask ci`, its documentation check included, so each changes the documents that describe its code:

- Move the shared pieces out of `verbatim-outpost`: the message layer, launch arguments, alarm, clocks, held processes and the children's start and end to `verbatim-process`; `verbatim-reader-rules`, with the joined grace and UIA focus wait; `verbatim-win32`, with `provider.rs` and `verbatim-ia2`'s window functions; the `WinEvent` thread and `fact_of_win_event` to `verbatim-ia2`. `verbatim-outpost` still holds the three programs, its cycles gone. With it: the new crates' guides, `verbatim-process.md`, `verbatim-ia2.md`, `verbatim-uia.md`, the index, and every document naming a path it moves, this one included.
- Split by role: `verbatim-outpost-logic`, `verbatim-listener-logic`, `verbatim-listener` with its executable, `verbatim-supervisor`, `verbatim-core`'s `supervisor` module, and every runner and script that names an executable, in one commit, since the executable and its staging change together. With it: the three role guides, `verbatim-core.md`, `docs/architecture.md`, `docs/tooling/`, the remaining documents naming moved paths, and this section reduced to its built form.
- The tier check in `cargo xtask ci`, with `CLAUDE.md`'s tiers and commands and `docs/tooling/setup.md`.

The builder runs targeted tests and `cargo xtask docs` while working and `cargo xtask ci` once at the end, checking each commit's tree with it before the next is made. The measurements before are taken first; then the end-to-end runs and the measurements after.

### Limitations

- Splitting the image costs the listener its sharing of code pages with the outposts, measured above.
- Logic that is pure in parts stays in the Windows crates, as listed above, outside the Linux build and the generated tests until moved.
- Window queries that behave alike stay as separate functions in `verbatim-win32`: `provider.rs`'s `parent_window` beside the general `parent`. Joining them is a follow-up.
- `verbatim-uia` and `verbatim-ia2` reach `verbatim-core` through `verbatim-reader-rules` for one type, `ForegroundRelation`, so they no longer compile alongside it; `cargo xtask ci`'s measured time shows the cost, and moving that type out of `verbatim-core` is the remedy if it matters.

### Decisions

- The seal stays in `verbatim-supervisor`: the generation's typestates in `verbatim-core` return a plain `Lifecycle` notice, and only the owner makes `Sealed`, which keeps the guarantee the seal gives (Dickson, 2026-10-31).
- `provider.rs` moves from `verbatim-uia` to `verbatim-win32`, re-exported by `verbatim-uia` under the same names, so the window queries have one crate under both backends (Dickson, 2026-10-31).
- `verbatim-listener.exe` and `verbatim-outpost.exe` sit beside `verbatim.exe`, found as the outpost and the synthesizer host are found today (Dickson, 2026-10-31).
- `verbatim-ext-api` is classed platform-neutral, so an extension's SDK never carries the GPL tier (Dickson, 2026-10-31).
- The `Arbitrator`'s pure decisions stay in `verbatim-outpost` in this build; M4.6's design decides whether its tests need them (Dickson, 2026-10-31).
- After the build, Dickson runs `vm\scripts\Enable-VerbatimCrashDumps.ps1` once, elevated, so `verbatim-listener.exe`'s crash dumps are kept.

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
- `LINUX_CRATES` before the split: the whole platform-neutral tier (`verbatim-model`, `verbatim-text`, `verbatim-speech`, `verbatim-config`, `verbatim-i18n`, `verbatim-input`, `verbatim-audio`, `verbatim-core`) and `xtask`, as the decisions below record. `xtask` is listed so the unit tests of the documentation check and the scans run on Linux. After the split: add `verbatim-outpost-logic`, `verbatim-listener-logic` and `verbatim-reader-rules`.
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
- Expected time: on this machine (12 threads, debug, cold), building the tests of core and its dependencies took 31 s and running them 66 s. The Linux job also runs Clippy, a separate check build of the same dependencies (ICU, `jieba-rs` with `zstd-sys`, the `workspace-hack` set), and `cargo tree --target all` once per platform-neutral crate, which downloads every target's crates on a cold cache, all on `ubuntu-latest`'s 4 processors, so a cold run of 6 to 8 minutes is as likely as 4 to 6. The Windows `ci` job took 8.2, 8.7 and 9.5 minutes in the last three successful pushes and `ci-arm64` 9.9 to 12.4, and the `e2e` job 60 to 67, so running beside them adds no wall time. The first runs give the real figure, and the design records it then. The Windows `ci` job's whole-workspace caches are 349 to 478 MB each, so a Linux cache for a few crates is likely well under a gigabyte; the repository uses 4.4 GB of its 10 GB Actions cache.

#### Before and after the split

- Before the split: xtask's gating, `--target all` in the dependency check, `ci`'s host-dependent steps with `LINUX_CRATES` holding the platform-neutral tier and `xtask`, the `ci-linux` job, and the `docs` job on Linux. None depends on the split.
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

## The outpost's program bookkeeping (a build of its own)

Status: proposed on 2026-10-31, and revised after its adversarial review (`experiments/logic-tests-study/bookkeeping-design-review.md`). The console poll's rule waits on a measurement, a capture and a question for Dickson. The rest does not depend on the answer except where said.

The build removes lifecycle states from the outpost's programs ([logic tests](logic-tests.md), "The outpost's program bookkeeping simplified"). It comes after the crate split: the reducer's changes land in `verbatim-outpost-logic` (today's `crates/verbatim-outpost/src/outpost/reducer/`), and the shell, worker, inbox, commit and request context stay in `verbatim-outpost`. Paths and lines below are today's.

### Where the code contradicts the logic-tests design

- Programs already get fresh copies at dispatch: `Reducer::dispatch` (`reducer/mod.rs` line 1438), `dispatch_finish` (1488) and `dispatch_poll` (1507) each clone `self.facts`. The worker keeps nothing between programs but the console poll's carry (`worker.rs` lines 289 to 300, `carry` at 319, `RequestContext::take_program_text` in `outpost/mod.rs` at 570). `judged_focus` (`worker.rs` 4386) reads the program's dispatch-time copy of the reports, not a copy the worker holds. So that part is mostly a consequence of the console poll's rule, plus moving one judgement into the reducer.
- Verdicts already travel alone: `RequestContext::record` (`outpost/mod.rs` 619) posts each verdict as a commit holding nothing else. The exception is in the types: `Effects::verdicts` (`commit.rs` 187), `Change::verdicts` (`reducer/follow.rs` 450), the verdicts branch of `admit` (`shell.rs` 391), and the reducer recording verdicts before it checks whether the commit is accepted (`reducer/mod.rs` 529). The change splits the type and changes no behaviour.
- A program commits more than once, so only its last commit could travel with its end. That part is dropped (part five).
- A test settle and the idle query differ in four ways:
  - A settle waits for the whole intake to empty, later arrivals included (`reducer/mod.rs` 1357 requeues it while `Intake::busy`); the idle query waits only for what came before it.
  - The idle answer waits for an open caret watch and an open gap record; a settle does not.
  - A settle's program waits for the subscription's moves and the outbound writes.
  - The idle query ends its batch; a settle does not.

  The merge keeps one item and one way of reaching it, and answers a test's settle as soon as it is reached (part one).

### Part one: test settles merged into the idle barrier

Today: `Outpost::settle` (`outpost/mod.rs` 1314) posts `Posted::Settle(Sender)`. The shell numbers it in `Settles` (`shell.rs` 247: a `waiting` map and the `in_flight` slot) and pushes `Item::Settle(id)`. The reducer reaches it in `run` (`reducer/mod.rs` 1357), requeues it while the intake is busy, and dispatches `Work::Settle(id)` (137). The shell gives the job the waiter (`Job::settle`, `worker.rs` 101). The program calls `Registration::settle` and `Outbound::flush`, then sends on the waiter (`worker.rs` 416). The shell lets go of the waiter on the program's `Finished`. It also lets go through `Settles::in_charge` (`shell.rs` 791) when the program's generation is dead and the settle was not dispatched again. A settle dispatched behind the console poll is dispatched again if the poll is abandoned, and gets the same waiter; that re-dispatch is where the waiters were lost twice.

Proposed:

- One intake item, `Item::Barrier { asker, behind_noted }`, where `asker` is `Asker::Round(u64)` (Core's idle query) or `Asker::Settle(u64)` (a test's). The item ends a batch, and a noted caret goes just ahead of it (`Intake::push_ahead`), as the idle query does today. Reason: one way of reaching the barrier, one ordering rule.
- A test's settle is answered when the reducer reaches it, with no program in flight: `Effect::Settled(id)`. A round is answered as it is today (`answer_idle`, `reducer/mod.rs` 1188). Reason: `crates/mockapp/tests/caret_watch.rs` settles 58 times, many with a watch open, and expects the settle to return while the watch stays open. With the round's conditions each such settle would wait 10 seconds (`CARET_WATCH_BOUND`) and then hear the watch end.
- The shell keeps the waiters in a map under the settles' numbers. After each transition it asserts that the map's keys are the `Asker::Settle` items waiting in the intake, which the reducer lists from the intake, as the shell already checks its reply tokens. The reducer keeps no set of its own. Under the second and third poll options (part four), a settle is answered in the same transition that reaches it, so the intake's items are exactly the pending settles. Under today's yield a settle reached while the poll is in flight would wait for the poll's end, which needs a list of reached settles in the reducer. Reason: a second record of the same fact would be one more thing to keep equal.
- `Outpost::settle` waits for the answer, then calls `Registration::settle`, `Outbound::flush` and the event thread's `sync` on the caller's thread. Every subscription move and message before the answer was handed to its thread before the shell applied `Settled`, so these calls come after them in their channels. Reason: waiting is the test thread's job, and a reducer thread that waited would stall the outpost.
- Removed: `Work::Settle`, `Item::Settle`, the `Settles` slot and its `in_charge` cleanup, `Job::settle`, the requeue while busy, and the re-dispatch behind the poll. Added: none.

Each request gets one answer:

- Settle `s` is pushed while program `p` is in flight. `p` posts `Finished`, the pump plans the intake, and the reducer reaches `s` and emits `Settled(s)`. The shell removes `s`'s waiter and sends on it. No program runs for the settle, so nothing can strand it.
- `p` is abandoned while `s` waits. `judge_abandonment` starts the replacement and the pump goes on, so `s` is reached and answered as above. A generation's death touches no settle.
- A noted caret is pending when `s` is reached. The caret and `s` are put ahead (`behind_noted`), the caret's program runs, and `s` is reached again and answered. A caret noted by that program is not waited for, as with the round today. Today's settle took noted carets each time it was reached.
- `Close` comes before `s` is reached. `Intake::close` drops the item, and the shell drops every waiter as it takes `Close`, so `Outpost::settle`'s receive fails and it returns, as it does today at shutdown. Dropping the waiter is its answer. A `Posted::Settle` after the mailbox has closed is dropped with its waiter (`shell.rs` 139 to 152).

Behaviour: Core's round is unchanged, so production is unchanged. In tests a settle:

- no longer waits for entries that arrive after it;
- no longer returns early when a worker is abandoned;
- divides batches, so events on either side of it are not merged or reordered across it;
- leaves the objects the shell let go of to be dropped with the next program rather than with the settle's.

Wake-ups and processor time: no change in production, since Core asks idle questions only of the harness. In tests each settle costs one shell wake and no worker wake. Today it costs a shell wake, a worker wake, and a shell wake for its `Finished`.

Guarded by:

- the replay tests `the_idle_answer_waits_for_nothing_that_arrived_after_its_query`, `a_noted_caret_is_read_with_its_events_timing`, `the_idle_answer_waits_for_the_hooks_copies_held_at_the_start`, and `a_gap_read_dispatched_again_after_an_abandoned_poll_still_holds_the_idle_answer`, which becomes "a gap read in flight holds the idle answer";
- a new replay test, "a settle is answered as it is reached, and a close drops it".

The shell tests `a_settle_dispatched_again_keeps_its_waiter` and `an_abandoned_settle_replaced_by_another_lets_its_waiter_go` go with the table. The settle's real path runs in the mockapp suites that settle: `caret_watch.rs`, `call_counts.rs`, `focus_reports.rs`, `msaa_events.rs`, `terminal_reading.rs`, `combo_box.rs`, `remote_ops.rs` and `idle_barrier.rs` under `crates/mockapp/tests/`. The end-to-end scenarios run only the round's path.

### Part two: verdicts posted as a message of their own

Today: a verdict is a `Change::verdicts` entry, posted alone by `RequestContext::record`. `admit` keeps it out of a refused commit, and `Input::Commit` records it before checking `accepts`. A dead generation's verdict is recorded only when it is a timed-out probe or a window's forgetting (`VerdictChange::kept_from_any_generation`).

Proposed:

- `Posted::Verdict { generation, change }` becomes `Input::Verdict { generation, change }`, recorded by `record_verdicts` under the same rule. `Effects` and `Change` lose `verdicts`.
- `Posted::Verdict` is taken after the mailbox closes, as `Commit` is (`Mailbox::post`, `shell.rs` 137 to 156). Reason: a probe that timed out during shutdown is still a fact.
- `admit` refuses whole any commit that is not accepted, and the reducer is given no `Input::Commit` for it. Reason: a refused commit is refused with no exception, so M4.6's model of commits needs no carve-out.
- The program's copy still takes the verdict at once. A publication's context, which runs no program, still keeps the verdict in its copy only.
- Removed: `admit`'s verdicts branch and the verdicts field. Added: one message kind and no state.

Each verdict gets one answer. Worker generation `g` probes window `h`, times out, and posts `Verdict { g, change }`. If `g` is in charge, or the change is kept from any generation, the reducer records it, and `Effect::Verdicts` replaces the mailbox's copy. The program's next commit, request `r`, is applied if `accepts(r)`; if not, it is dropped whole and its objects go to the garbage. A probe committed before its worker's death keeps its mailbox order ahead of the abandonment's effects, as today.

Wake-ups and processor time: unchanged, since the post is the same.

Guarded by: `a_probe_is_recorded_when_committed_and_reaches_the_event_thread_then_the_next_program`, `a_dead_generations_probe_is_not_recorded`, `a_dead_generations_timed_out_probe_is_recorded`, `a_timed_out_probe_committed_before_its_worker_is_abandoned_stays_recorded`, and the shell test `a_refused_commits_text_writes_are_not_made`.

### Part three: the worker keeps nothing between programs

Today the worker keeps the carry described above. A program dispatched behind the console poll was copied before the poll's commits were applied. So the worker applies the poll's kept commits (`RequestContext::keep_commits`, `kept_commits`) to the next program's copies, and hands on the store text the poll held (`ProgramText`). Separately, `judged_focus` decides `entered_in_front` on the worker:

- a foreground change sets it to its window;
- any other focus inherits it from `Held::newest_in(top_level)` on the program's copy;
- with nothing to inherit, it is set when the top-level window is the foreground window as read.

That is a judgement written into the reducer's fact, made on the worker.

Proposed:

- The carry goes with the console poll's slot. Under the second and third options of part four, nothing is dispatched while a poll is in flight; today's yield keeps the carry. Removed: `carry`, `keep_commits`, `kept_commits`, `Program::kept`, `take_program_text`, and `for_program`'s text argument.
- The program reports evidence, not the decision. `reducer::Focus` carries `in_front: InFront`, where `InFront::Foreground(isize)` is a foreground change's window, and `InFront::Read(Option<isize>)` is any other focus's top-level window when it was the foreground window as read. The reducer's rule, in `held.reported` (`follow::record`, `reducer/follow.rs` 841):
  - a foreground change sets `entered_in_front` to its window;
  - any other focus inherits it from the newest report in the same top-level window;
  - otherwise it is set from the read.

  These are today's three cases in today's order. A foreground change is named by its variant, not by its event address, since its address is the same as that of an MSAA focus on the window's client object (`foreground_event`, `worker.rs` 609). The same applies to the publication at a walk's bound (`worker.rs` 4630) and to `Report::Answered`. Reason: the decision becomes a pure function in the logic crate, where replay tests and M4.6's model check it.
- The program's local apply runs the same pure function on its copy, so its later reads agree. It is still judged in two places, from the same evidence and the same function. The two agree at each commit, because the program's copy of the reports equals the reducer's then. The reports are written only by:
  - the program in flight's own commits;
  - the publication at its walk's bound, which claims the program, so none of its later commits is applied;
  - `Input::Released`, which comes only while no program is in flight: `NodesHeld` is reached only with none in flight, and `releasing` holds dispatch until it is done.

  The console poll writes no report.
- What is heard does not change: each of today's three cases gives the same value. The roadmap's follow-up, a foreground change updating older reports in the window, stays an item of its own.
- Cost: the foreground window is read on every non-foreground focus report whose top-level window is known, rather than only when nothing is inherited. This includes the shell thread's publication at a walk's bound. It is a local read with no wake. Its time is not measured; the build's before and after figures cover it.
- Removed: the worker's state between programs. Added: none.

Guarded by:

- `a_programs_text_change_reaches_the_next_programs_copy`;
- `a_walk_under_way_at_its_bound_is_published_and_its_programs_report_refused`;
- `a_release_waits_for_the_program_in_flight_and_names_no_dead_request_live`;
- two new replay tests: "a focus inherits its window's entered-in-front at commit", and "a foreground change after a background focus in the same window sets it".

### Part four: the console poll (a question for Dickson)

Today (`reducer/mod.rs`): the poll is a long program (`Work::Poll`, line 143) in a slot of its own, `Reducer::poll` (398), dispatched when nothing else is ready.

- A program dispatched meanwhile goes into the same worker's inbox behind the poll, its deadline held back (`Current::behind_poll`, 365).
- The poll looks at its inbox before each read (`look`, `worker.rs` 335). It yields to anything but a release's drop (`poll_yields_to`, 328), commits `WatchChange::Yielded`, and is dispatched again afterwards.
- When the poll is abandoned, its watch is taken, its query is answered `Abandoned`, and the program behind it is dispatched again to the replacement (`abandon_poll`, 1042). `poll_finished` (1152) starts that program's deadline.

The states are the second slot, `behind_poll`, the re-dispatch after abandonment, the yield, the worker's held queue and the carry.

NVDA, from the pinned source (`nvda/source/editableText.py`, `_hasCaretMoved`): the caret loop runs on the main thread until its timeout, 100 ms by default. It returns early only for a queued script or a pending focus event. In consoles, caret and text change events neither end it nor are handled during it, since `caretMovementDetectionUsesEvents` is false there.

#### What is known

- `docs/design/key-holding.md` ("The console host's caret poll", Costs) measured, under today's yield, that every watch the first check left open was answered within 8 ms. The answer came from one re-read, or from the check of a console update that ran because the poll yielded. The workload, `measure_keys_conhost`, presses only keys that move the caret: Left and Right over a typed line, Backspace, Escape and typing (`crates/verbatim-e2e/src/scenarios/measure.rs`, `terminal_keys`). So nothing is known of keys that move nothing, which pay the poll's whole 50 ms bound.
- The blink-timer experiment saw the cursor move up to 54 ms after the key, past the bound. A slow move also pays the whole bound, then waits for the blink-timer caret event, 257 to 540 ms after the key.
- How often a program arrives during a poll, and how long it would wait without the yield, is not measured.

#### The order of reads when a character is typed during a poll

Take this sequence in the console host:

1. Left Arrow is pressed at the start of the line. Its first check finds nothing, and the watch `q` opens with a poll.
2. At about 10 ms, "a" is typed. A key that is not held is not made to wait for the watch (`crates/verbatim-core/src/editing.rs`, `WaitingKey::Later`).
3. The console inserts "a", and its caret moves one place right. The console update `U` arrives.

What Core receives depends on the option:

- Today's yield: `U`'s program checks the watch, finds the caret moved, and answers `q` as moved, with the caret after "a", under the key's trace. `U`'s read of the terminal, which holds the "a", is sent as an event under `U`'s own trace. One exception: when a poll read in flight at `U`'s arrival finds the move first, the terminal read goes under the key's trace.
- The poll waits for nothing: `U` waits. The poll's next re-read finds the move and answers `q` the same way. The poll then reads the terminal under the key's trace (`terminal_output(node_id, open.trace, 0)`, `worker.rs` 1235 to 1237), so the "a" first reaches Core as part of the key's result. `U`'s read then finds nothing new. The order changes in every such case, not only in the exception.
- Each read a program: `U` is planned before the next re-read, since a re-read is dispatched only when nothing else is ready. Core receives what it does today, the same exception included.

How Core's terminal reading rules speak the "a" in either order is not established, and the expected speech must come from a capture. The capture needed is NVDA in the console host running cmd at an empty prompt: Left Arrow, then "a" 10 ms later, ten times, and the same after a Left Arrow that moves over typed text. No scenario types within the poll's bound after a key: `conhost_up_typing` types only after the recalled line is heard, and `conhost_typing` is recorded as flaky. A scenario of its own, `conhost_unmoved_key_typing`, would assert that capture's speech. It is built only if the poll waits for nothing is chosen, since under the other options Core receives what it does today.

#### The options

For each: the longest wait for a program that arrives during a poll (a focus, a console update's read, a release), the wake-ups, and what is removed.

- Today's yield.
  - Longest wait: one read in progress, then the program.
  - Wake-ups: the worker once per re-read, plus one or two shell wakes per yield, and the re-dispatch.
  - Removed: nothing. It keeps every state listed above, and part three's carry with it.
- The poll waits for nothing: the poll is an ordinary program in the one slot, and whatever arrives waits for its end.
  - Longest wait: until 50 ms after the key was passed on (`POLL_BOUND`), plus one read. That is paid in full after a key that moves nothing or moves later than the bound, and is otherwise unmeasured.
  - Wake-ups: the worker once per re-read, and none for yields.
  - What Core receives changes, as above.
  - Removed: `Reducer::poll`, `behind_poll`, `abandon_poll`, `poll_finished`, `WatchChange::Yielded`, `look`, `poll_yields_to`, the worker's held queue and the carry.
  - The poll still waits on its inbox's high-resolution timer, and needs a `try_take` after each wait to see that its inbox closed.
- Each read a program: the watch's next read becomes its wake (`wake_for` returns `poll.next` for a polling watch). `Item::Wake` on a polling watch makes one read and commits the answer, the end, or `WatchChange::Read` with the next read's time. A wake taken before that time only sets the wake again, as `Worker::wake` does today. The re-read runs under the text deadline.
  - Longest wait: one read in flight.
  - Wake-ups: three or four per re-read, for the shell's timer, the worker, and the shell for the commit and the end, against one today. A key that moves nothing makes ten re-reads, so 30 to 40 more wakes.
  - Processor time: one more copy of the facts per re-read.
  - What Core receives does not change.
  - Removed: everything the previous option removes, plus the long program and the inbox's timer, so the inbox becomes a plain channel and the worker waits on it with `recv`.

With any option, a read can last until the program's deadline, 10 seconds, since the poll names no window for the moved-on rule. Today a program behind the poll waits the same way.

Each request gets one answer: watch query `q` is opened by a check, and the poll's program `P`, or each re-read program under the third option, runs with `running = q`.

- `P` finds evidence. Its commit carries `q`'s reply and `Answered(q)`, the reducer's `send` clears `running`, and `P` ends.
- `P`'s last read finds nothing. `Ended(q)` ends the watch, and `subscription_effects` sends `q`'s `WatchEnded`.
- Under the third option, a read that finds nothing before the bound commits `Read(q, next)`. That sets the watch's poll and wake, and `q` stays unanswered with the watch open. The idle round waits for it as for any open watch.
- `P` is abandoned at its deadline. A new rule for every program, moved from `abandon_poll`: when the running query is the open watch's, the watch is taken out without a word and the query is answered `Abandoned`, once. `P`'s later commits are refused, since its slot is empty. An ordinary program that took the watch has already removed it from the reducer's facts, so the rule does not fire for it.
- `P` panics. A rule in `finished`, moved from `poll_finished`: when the running query is the open watch's, the watch is ended, which answers `q`, and no `Failed` is sent. Otherwise the query is answered `Failed`, as today. Ending the watch and then sending `Failed` would answer `q` twice, so the order is fixed by this rule.
- A newer key's query arrives. It is planned ahead of any wake, and its check ends a watch still open, as today. A wake left for the ended watch finds no polling watch and does nothing.
- `Close` arrives. The shell lets go of the generation's inbox and `P` returns. Its end is taken as any program's.

Guarded by:

- kept: `the_console_poll_is_dispatched_only_when_nothing_else_is_ready`, `a_newer_keys_request_ends_the_old_watch_and_its_poll`, `the_poll_ends_its_watch_by_its_answer_or_at_its_last_read`, `a_query_is_answered_once_whatever_ends_it`, and `the_poll_ends_when_its_inbox_is_closed` under the second option;
- rewritten: `a_program_is_dispatched_beside_the_poll_its_deadline_starting_when_the_poll_ends` becomes "a program waits for the poll's end" under the second option and "a program goes before the next read" under the third; `an_abandoned_poll_ends_its_watch_and_the_program_behind_it_goes_to_the_replacement` becomes "an abandoned poll answers its watch's query once";
- deleted with their states: `the_poll_is_dispatched_again_after_it_yields`, `the_poll_holds_a_drop_and_goes_on_reading`, `the_poll_yields_to_a_caret_read_and_holds_it`, and `the_polls_text_changes_are_committed_with_its_yield`;
- new: a replay test for a panicked poll, and under the third option "a read before the bound sets the next read's wake".

#### The measurement that settles it

Run it on today's code, in release builds and test audio mode, on the desktop. A builder makes two temporary changes, not committed:

- A debug log line in the reducer for each poll. It gives the time the key was passed on and the poll's `until`. For each program dispatched behind the poll, it gives the program's kind, its offset from the key, and its offset to the poll's yield. For the watch, it gives the time it was answered or ended, and whether a poll re-read or another program's check did it. It also gives the number of re-reads.
- A temporary workload, `measure_keys_conhost_unmoved`, in `crates/verbatim-e2e/src/scenarios/measure.rs`, against Windows PowerShell in the console host as `measure_keys_conhost` uses. Each round presses, each key 150 ms after the speech of the one before:
  - Left Arrow 20 times at the start of an empty line;
  - Home 20 times at the start of the line;
  - Up Arrow 20 times in a fresh shell with no history;
  - Left Arrow at the start of an empty line, then "a" 10 ms later and Backspace, 20 times;
  - as a control, Left and Right Arrow 10 times each over a typed line.

Run `measure_keys_conhost_unmoved` 10 times and `measure_keys_conhost` 10 times, and record each run's time from its log.

From each run, take:

- for each program that arrived during a poll: the time from its arrival to the poll's `until` (its wait if the poll waits for nothing, at most) and to the watch's answer (its wait if the answer comes first), as P50, P99 and maximum, by program kind;
- the share of polls that end at the bound, split into keys that move nothing and keys whose caret moved later. The caret moved when the console's next caret event or the next key's before read shows a new caret position;
- re-reads per poll, as P50, P99 and maximum, which gives the third option's added wake-ups at three or four each;
- the reducer's and the worker's wake-ups per key from a trace, as `docs/tooling/measuring.md` says, in one run of each workload.

### Part five: dropped

Posting a program's last commit with its end is dropped. The hop has not been shown to cost. Posting the end before the program's drops would put a blocked release under the next program's deadline, and would let a settle or the idle round be answered while the worker still calls into the application. If a later wake-up measurement shows the hop costs, the end would carry the last commit and still be posted after the drops.

### Facts with an owner in two places

- The reducer's verdicts and marks, and the mailbox's push-time copies of them: unchanged, each still written only from the reducer's effects.
- Pending settles: the barrier items in the intake, and the shell's waiters, checked against those items after every transition. The waiter is only the means of answering, as a reply token is.
- The console poll's progress, under the second option: the watch in the reducer keeps the poll as it was at dispatch, while the program holds the live one. Nothing reads the reducer's copy while the poll is in flight, since `wake_for` gives no wake for a polling watch, and the program's end replaces the watch. Under the third option the progress lives only in the reducer.
- `entered_in_front`: judged by the same pure function on the program's copy and in the reducer, from the same evidence, and equal at each commit by the argument in part three.

### Measured before and after

Wake-ups and processor time per thread, in release builds and test audio mode, at most 20 runs each:

- the console poll's option on `measure_keys_conhost` and `measure_keys_conhost_unmoved`;
- part three and the build as a whole on `measure_focus_text_box` and `measure_keys_notepad`.

### End-to-end runs and the order of work

The commits, in order, each with its unit and replay tests, and the crate guide updated where an API changes:

- Verdicts as a message of their own. Scenarios: `quick_link_menu`, `menu_and_settings_dialog`.
- The console poll by Dickson's answer, with the worker's carry removed and the one-answer rules for an abandoned or panicked poll. The ownership design's limit on a release waiting behind the poll is updated. Scenarios: `conhost_up_typing`, `conhost_editing`, `conhost_commands`, and `conhost_unmoved_key_typing` if the poll waits for nothing is chosen.
- Test settles merged into the idle barrier. Its gate is the mockapp suites that settle, named in part one, which `cargo xtask ci` runs. Scenarios: `notepad_caret_keys`, `text_box_caret_keys`, for the round's path.
- `entered_in_front` decided by the reducer. Scenarios: `second_application_and_verbatim_menu`, `rapid_tabbing_in_settings`.

Each scenario runs three times at the end of the build. `conhost_typing` is left out, since `docs/flaky-tests.md` records its failure.

### Questions for Dickson

- The console poll. Should it keep today's yield, wait for nothing, or make each read a program?
  - What is known: under today's yield, every caret key that moved the caret had its watch answered within 8 ms, partly by checks the yield let run. The workload had no key that moves nothing, and the cursor has been seen to move up to 54 ms after a key.
  - What the measurement adds: how long programs that arrive during a poll would wait under the second option, how often a poll runs to its bound, and the third option's wake-ups per key.
  - What the capture adds: whether the second option's change in order, the typed "a" read first as the Left Arrow's result, changes what is spoken.
  - Recommended: each read a program. It removes the same states and the inbox's timer, keeps what Core receives exactly as today, and keeps the wait for any program to one read. Its cost is 3 to 4 wake-ups per re-read, paid only on console caret keys the first check finds unmoved. Choose the poll waits for nothing instead only if the measurement shows those wake-ups matter, the waits it adds are small, and the capture shows the order does not change what is heard.
- A test's settle. Should it be answered as soon as it is reached, without the round's waits for an open watch, an open gap record or the hook's held copies, dividing batches as the round does? Recommended: yes. The caret watch tests settle with a watch open, and the round's waits would make each such settle wait 10 seconds.
