# The end-to-end harness and the agent: merged and verified record

Area: `crates/verbatim-e2e/src` without `scenarios/`, `scenario.rs` and `registry.rs`; `crates/verbatim-e2e/tests/e2e.rs`; `crates/verbatim-agent`; `nvda-addon/`; the capture part of `xtask/src/nvda.rs`. Reviewers A and B. Every quotation, count and commit below was checked against the tree at ba89dca7 on 2026-11-20 with reads, grep, `wc -l` and `git show --stat`; nothing was built or run.

## 1. Size

Recounted with `wc -l`, each file split at its first `#[cfg(test)]`.

| Part | Product | Tests | NVDA counterpart (pinned 732d1ce2) | Ratio | What the extra lines buy |
|---|---|---|---|---|---|
| Speech collection and assertion (`speech.rs`) | 2,032 | 1,094 (42 tests) | `nvda/tests/system/libraries/SystemTestSpy/speechSpyGlobalPlugin.py` 599 (settling `SPEECH_HAS_FINISHED_SECONDS = 1.0` at 57; joining at 225 to 228; waiting for the core to sleep at 550 to 555) and `AssertsLib.py` 106: 705 | 2.9x | Endings (heard in full or cut off), the optional-utterance matcher, the idle barrier, the failure report with trace lines, latency rows, and compare mode's hooks (70 lines of `speech.rs` mention `compare`). |
| Agent and its client (`verbatim-agent` 22 files; `agent_client.rs`) | 8,557 + 1,638 = 10,195 | 1,937 (64 tests) + 0 | `NvdaLib.py` 476, `WindowsLib.py` 176, `SystemTestSpy/windows.py` 168, `NotepadLib.py` 187, `ChromeLib.py` 298, `blockUntilConditionMet.py` 77: 1,382 | 7.4x | The TCP agent so the suite drives a Hyper-V guest and GitHub's runner from outside; job-based crash and exit tracking; independent state reads; the scenario-window baseline; the guard on the owner's own Windows Terminal; wait chains; the DXGI capture. About 900 lines buy nothing (F3). |
| Recording (`recording.rs`, agent `capture.rs` and `pace.rs`) | 715 + 297 + 89 = 1,101 | 152 | none | n/a | A video of every run. |
| Compare mode and the NVDA client (`compare.rs`, `nvda.rs`, the add-on, `xtask/src/nvda.rs`) | 492 + 540 + 214 + 1,005 = 2,251 | 324 + 287 + 136 = 747 | none: NVDA's system tests assert hand-written expectations | n/a | See section 4, question "Compare mode". `nvda.rs` and the add-on also serve `cargo xtask nvda capture`. |
| Artifacts, timeline, run summary, command line, lib, Windows Terminal staging | 693 + 500 + 44 + 57 + 86 + 208 = 1,588 | 341 + 140 + 20 + 64 + 37 = 602 | none (Robot Framework's report and `nvda.log`) | n/a | The failure report, trace lines, history archive, portable Windows Terminal. |
| Test binary (`tests/e2e.rs`) | 301 | 0 | `robotArgs.robot` and the runner | n/a | A second scenario list (F2). |
| Total | 17,082 | 4,532 | NVDA's system-test libraries 2,658 (robot suites another 5,478) | 6.4x the libraries | |

Reviewer B's totals (about 17,000 and 4,500) are right; A's agent test count (57) is 64.

Deletion target, from the findings in section 2:

- Remove now: about 1,700 product lines and about 560 test lines (client typed replies and launch value 900; test-binary list 150; summary as JSON 120; NVDA transcript push and serde parsing 150; overlapped pipe once 150, not recounted; collector record 60; frames read once 60; dead code and changelog 100; dispatch nesting 40; pin check 30; jobs map 30; held keys 20; defect marker 30 net. Tests: protocol round trips 330; summary tests 80; poller tests 287 replaced by about 120, net 160; dead timeline tests 40; trivial path tests 25; the defect list's self-test 10; minus about 85 to add).
- If Dickson rules for the removal: compare mode, about 1,500 product and 650 test lines (the reviewers' estimate; `compare.rs` alone is 492 and 324, and the rest is spread through `speech.rs`, `nvda.rs`, `scenario.rs` and `registry.rs`). Not counted above.
- Planned phase (M5 extensions): the optional-utterance matcher shrunk to one slot once Windows Terminal's app module lands, about 120 product and 250 test lines (B's estimate, not recounted).

## 2. Findings

| id | title | class | found by | file and line | what goes | product saved | test saved | speech changes |
|---|---|---|---|---|---|---|---|---|
| F1 | The logged-defect gate no longer catches two of its 23 messages | Defect | both (drift named by A) | `crates/verbatim-e2e/src/artifacts.rs` 467 to 492, 505 to 512, 1024 to 1033 | the hand-copied list, for one marker | 30 net | 10 | no |
| F2 | The test binary keeps a second scenario list, held equal by a test reading source as text | Remove now | both | `crates/verbatim-e2e/tests/e2e.rs` 39 to 190; `registry.rs` 2849 to 2883 | `scenario_tests!` and its three lists; the agreement test | 150 | 45 (other pair's file) | no |
| F3 | One hand-written client method per agent request, matched by name at run time | Remove now | both | `crates/verbatim-e2e/src/agent_client.rs` (55 `Err(unexpected(` arms; 7 launch methods) | per-request methods, `unexpected`, `ReplyPayload` pairing | 900 | 60 (server tests' own client) | no |
| F4 | A failed focus read is answered as "the window's client has the focus" | Defect | both | `crates/verbatim-agent/src/focus.rs` 267 | the default | 0 (adds 5) | 0 (adds 1 test) | no |
| F5 | A failed or mis-sized frame copy leaves a stale frame that the recording check passes | Defect | both | `crates/verbatim-agent/src/capture.rs` 229 to 251 | the ignored results | 0 (adds 15) | 0 (adds 1 test) | no |
| F6 | NVDA transcript replies read missing fields as empty, zero or "nothing new" | Defect | both | `crates/verbatim-e2e/src/nvda.rs` 126 to 131, 166 to 167, 180 to 181, 312 | defaults, for serde types (and, B, a pushed transcript instead of polling) | 150 (with push) | 160 net | no |
| F7 | Two definitions of "the running NVDA is the pinned build" | Defect | both | `xtask/src/nvda.rs` 666 to 694; `compare.rs` 330 to 377 | one function | 30 | 10 | no |
| F8 | Run summary is a hand-made text format with a hand-made parser and old-format compatibility | Remove now | both | `artifacts.rs` 184 to 230, 584 to 690 | text format, parsers | 120 | 80 | no |
| F9 | Protocol compatibility defaults the handshake makes dead, and a changelog in a doc comment | Remove now | both (changelog B only) | `crates/verbatim-agent/src/protocol.rs` 20 to 95, 177 to 182, 11 `#[serde(default)]`; `server.rs` 149 to 152 | 11 attributes, 75 lines of history, one test | 80 | 20 | no |
| F10 | One utterance's facts in seven collections; collector modes as two independent fields | Remove now | both (modes B only) | `speech.rs` 457 to 466, 481, 487, 632 to 642 | collections, for one record and one mode enum | 60 | 0 | no |
| F11 | Speech frames are turned into text three times | Remove now | B only | `speech.rs` 590 to 657 and 1934; `xtask/src/nvda.rs` 503 | two text maps | 60 | 0 | no |
| F12 | The overlapped pipe is written twice | Remove now | B only | `crates/verbatim-agent/src/tunnel.rs` 160 to 357; `crates/verbatim-control/src/server.rs` 394 to 619 | one copy | 150 (not recounted) | 0 | no |
| F13 | Dead code | Remove now | both | `speech.rs` 1474; `timeline.rs` 304 | `take_collected`, `Timeline::utterances` and its 4 tests | 25 | 40 | no |
| F14 | The testing standard and the add-on's docstring are stale | Remove now (documents) | both | `books/dev/src/reference/testing.md` 52; `nvda-addon/src/globalPlugins/verbatimTranscript.py` 7 | two wrong sentences | 0 | 0 | no |
| F15 | Agent dispatch matches each group twice, with fallbacks only a logic bug reaches | Remove now | A only | `crates/verbatim-agent/src/server.rs` 415, 437, 593, 685, 913, 951 | sub-dispatch fallbacks | 40 | 0 | no |
| F16 | A launch is recorded in two pid-keyed maps, one released by a five-minute sweep | Remove now (the merge only) | A only | `process.rs` 592 to 595, 224 to 236; `jobs.rs` 54, 299 | the second map | 30 | 0 | no |
| F17 | Per-connection held keys in a global | Remove now | B only | `server.rs` 55 to 70 | the static | 20 | 0 | no |
| F18 | A window whose image cannot be read is never counted as a foreign Windows Terminal | Defect | A only | `desktop.rs` 538; `terminals.rs` 126 | the default | 0 | 0 | no |
| F19 | Trivial path tests | Remove now | A only | `artifacts.rs` 864 to 888 | 3 tests | 0 | 25 | no |
| F20 | Latency measured in two places | Question for Dickson | B only | `speech.rs` 269 to 277; `scenario.rs` 2723; `xtask/src/perf.rs` 365 | one source, when budgets are built | 0 now | 0 now | no |
| F21 | Optional-utterance matcher sized for uses M5 removes | Planned phase: M5 extensions | B only | `speech.rs` 900 to 921, 1736 to 1748 | the matcher's multi-slot readings | 120 | 250 | no |

### F1. The logged-defect gate (both; drift found by A)

`artifacts.rs` 467 holds `pub const DEFECT_MESSAGES: &[&str] = &[`, matched with `.filter(|line| DEFECT_MESSAGES.iter().any(|message| line.contains(message)))` (508). It has 23 strings (B said 24). Its one test, `every_defect_message_is_found` (1024), builds each line from the listed string itself, so it cannot fail on drift.

Check: for every string, `grep -rF --include=*.rs` over `crates` and `xtask`, leaving out `crates/verbatim-e2e`:

- No longer matches any log line (2):
  - "the supervisor handed on a listener message that is no tell or answer, a logic bug": the log is now "...that is no tell, destroy or answer, a logic bug" (`crates/verbatim-app/src/main.rs` 1089), changed by bc61c881 (2026-11-19, "Send every window destroy to Core and its outpost...").
  - "an answer did not decode as its request's reply, the reader's fault": now `"a malformed answer: its result is no reply its request can have"` at `tracing::warn!` (`main.rs` 1683), changed by 75b5fb5c (2026-11-17 20:22, "Decode every answer a text request can have"), 23 minutes after f5a4c4df (19:59) added the gate and named this very message as its proof.
- Matches no source by design (1): "' panicked at ", the standard library's panic text.
- Still match a log call (20): "Core's front entry could not be rebuilt..." (`main.rs` 1690); the three flight-recorder messages (`flight_dump.rs` 160, 163, 166); "launched a child for a generation that is not launching" (`supervisor/owner.rs` 1395); "a write completed for a pipe the relay does not have" (`relay.rs` 231); "an outpost's message could not be read" (`frames.rs` 145, `owner.rs` 582); "the held nodes could not be encoded" (`writer.rs` 234); "a completion that is not this reader's" (`process/transport.rs` 625); "a frame could not be encoded and is skipped" (`transport.rs` 1340, 1370); "a route's end names an ancestor..." (`outpost/completion.rs` 86); "the outpost cannot watch another application" (`outpost/mod.rs` 928); "the worker failed running a program" (`worker.rs` 287); "a UIA event callback panicked" (`verbatim-uia/src/com.rs` 72); "the lock check failed: " (`outpost/lock.rs` 198, `win32/held_locks.rs` 145); the listener's three (`listener/asked.rs` 178, 708; `listener-logic/schedule.rs` 697); the two eSpeak NG messages (`synth-espeak/src/lib.rs` 304, 248).

Sequence: a reader answers a request with another request's reply kind; Core logs "a malformed answer: ..."; the speech still matches, the gate finds nothing, the run passes. Whether a malformed answer is still a Verbatim defect after 75b5fb5c (whose message says "The outpost was right; the contract was wrong") is for the fix to decide when it marks it. Fix: log every defect with one marker (a `tracing` target such as `verbatim::defect`) and match the marker and the panic text. Speech unchanged.

### F2. The second scenario list (both)

`tests/e2e.rs` 39 to 44 defines `scenario_tests!`, whose `$reason` is discarded (`&[$(stringify!($name)),*]`); 126 names follow in `LIVE` (48), `DEMONSTRATIONS` (160) and `MEASURE_SET` (173) (A said 129). `main` builds trials with `.map(|name| scenario_trial(name, &summary))`. `registry.rs` 2849 to 2883 reads the file with `include_str!("../tests/e2e.rs")` in `the_test_binary_has_a_test_for_every_scenario_and_no_other` (added 3e11bbf3, 2026-10-10). The binary is `libtest-mimic` with its own `main`, so the trials can come from `registry::SCENARIOS` without the disabled ones. Every new scenario stops touching `tests/e2e.rs`.

### F3. The agent client (both)

`agent_client.rs` is 1,638 lines with no tests. `grep -c "Err(unexpected("` gives 55 (A said 56, which counts the function itself). Example, 849 to 856: `match self.request(Request::ControlledItems { names })? { Frame::Reply { payload: ReplyPayload::ControlledItems { count, names }, .. } => Ok((count, names)), other => Err(unexpected("ControlledItems", &other)), }`. `grep -c "Request::LaunchProcess {"` gives 7; the variant has 10 fields, four of them flags (`minimized`, `withhold_foreground`, `ignore_foreign_terminals`, `stdin_piped`). Which reply answers which request is checked only at run time. Fix: a type per request with its reply as an associated type and one generic call, and one launch value with defaults. Dropped: A's sub-claim that `request()`'s `_ => {}` (169 to 171) can only be a logic bug (section 6).

### F4. The failed focus read (both)

`focus.rs` 267: `let focus = unsafe { client.accFocus() }.unwrap_or_default();`. An error becomes an empty VARIANT, which is neither `VT_I4` nor an `IDispatch`, so the loop takes `break (client, CHILDID_SELF.cast_signed(), focus);` (275). The doc at 248 to 250 promises "a failed read is never taken for an empty name or no state". `Scenario::desktop_speech` (`scenario.rs` 2054 to 2086) builds every scenario's startup expectation from this read: `"Desktop list"` then `format!("{}{state} {position} of {size}", focus.name)`, and the position is read from the focus window by list-view messages, not from the child (`list_view_position(window, hop_limit)`, 318).

What a run would show, settled between the reviewers: when the focus window is the desktop's `SysListView32`, the expectation becomes the list's own name with the focused item's position, and the startup step fails as a speech mismatch against Verbatim's correct item name (A's sequence). When the focus window is still `SHELLDLL_DefView`, `list_view_position` answers `NotListView` and the scenario fails with "the desktop item's position could not be read: ...; the focused object is named ..." (B's sequence, which said the folder view's name). Either way the failure blames Verbatim or the position, not the agent's failed read. Also on 269, `i32::try_from(&focus).unwrap_or(CHILDID_SELF.cast_signed())` inside `if focus.vt() == VT_I4` cannot fall back. Fix: propagate the error as the name read does.

### F5. The desktop capture (both)

`capture.rs` 229 to 251: `self.output = duplicate(&self.device).ok();`, then `let _ = self.copy(&resource, frame);`, then `Err(_) => self.output = None,`. `copy` calls `self.context.CopyResource(&self.staging, &texture)` with no result to check, under the comment "both textures are live and alike in size and format"; `self.size` and the staging texture are set once in `new`, so a re-duplication at another resolution is never compared with them. A frame left unchanged is also the designed answer for "no new frame", so a stale frame is indistinguishable. `verify` (`recording.rs` 587 to 640) checks mux errors, a clean decode, frame count against ffmpeg's encoded count, the last frame's time and coverage of the body's end. What a run would show: a video frozen from the loss onward (a secure-desktop switch, a mode change), a passing recording check, and nothing in any log. Fix: count failed copies, failed re-duplications and size changes, and fail the capture on any.

### F6. The NVDA transcript replies (both)

`nvda.rs` 127 to 130: `payload["Entries"]["entries"].as_array().map(...).unwrap_or_default()`; 166 to 167: `seq: entry["seq"].as_u64(), ms: entry["ms"].as_i64().unwrap_or(0),`; 180 to 181: `priority: speech["priority"].as_str().unwrap_or("?")`, `text: speech["text"].as_str().unwrap_or("")`; 312: `self.last_seq = entry.seq.unwrap_or(self.last_seq);` (and the same at `xtask/src/nvda.rs` 478). `request` (102 to 117) checks the add-on's `Error` but not the reply's `id`. The add-on today sends every field (`verbatimTranscript.py` 96, 103, 182), so nothing shows now; nothing tests the add-on's wire format against this client. What a run would show after a shape drift: a missing `entries` makes every poll empty, so `cargo xtask nvda capture` prints no NVDA speech after any step and settles after the quiet period, and compare mode reports every step that expects speech as "differs" (`Verdict::Differs`, `compare.rs` 288) and every step that expects none as "same"; a missing `text` gives an empty utterance; a missing `ms` gives time 0, which the timeline adds to the Hello time (355); a missing `seq` keeps `last_seq`, so the next poll reads the same entries again and NVDA's speech is doubled. The add-on also keeps `collections.deque(maxlen=MAX_ENTRIES)` with `MAX_ENTRIES = 10000` (51, 75), so the oldest entries drop with no sign to the client. Fix: serde types that fail on a missing field; B adds a pushed transcript in place of the 50 ms poll (`POLL_INTERVAL`, 50; the thread at 516 to 539; `thread::sleep(POLL_INTERVAL)` at `xtask/src/nvda.rs` 749).

### F7. The pin check (both)

`xtask/src/nvda.rs` 689 to 694 `runs_pinned_build` accepts only `version.rsplit_once(',')` with a commit prefix, and its test asserts `!runs_pinned_build("2025.3.1", pinned)` (1122). `compare.rs` 364 to 377 `pin_matches` also accepts a release whose tag is `release-<version>` and requires hex digits. Sequence: install the NVDA release the submodule's tag names; `cargo xtask nvda capture` and `cargo xtask nvda compare` (which calls `Source::nvda`, 603, which calls `require_pinned_nvda`, 427) refuse it, while a compare run started directly with `VERBATIM_E2E_COMPARE` reports the pin as matching. Fix: one function used by both.

### F8. The run summary (both)

`ScenarioSummary::write` (184) and `parse_summary` (611 to 676) with `parse_optional_count` (588: `value.parse().ok()`, so a malformed number reads as "unknown"), unknown keys skipped (`_ => {}`), and a compatibility comment for old summaries ("a summary written before they existed simply leaves them `None`"). Only reader outside the module: `xtask/src/vm/test.rs` 312 and 498. `serde_json` is already a dependency. Settled: B's inclusion of `utc_stamp`, `civil_from_days`, `parse_utc_us` and `days_from_civil` (297 to 345, 554 to 582) is wrong, since they name the history folders and read log timestamps, not the summary; they are a second copy of the calendar code in `xtask/src/durations.rs`, outside this area.

### F9. Dead compatibility and history (both; changelog B only)

`protocol.rs` 177 to 182 keeps `#[serde(default)]` "so an older client omitting this field still deserializes"; `grep -c "serde(default"` gives 11. `server.rs` 199 to 227 refuses any `Hello` whose `protocol_version` is not `AGENT_PROTOCOL_VERSION` and closes the connection, so a client of another version never sends a request. The test `launch_process_without_stderr_to_deserializes_as_none` exercises that path. `protocol.rs` 20 to 95 is a doc comment listing what versions 2 to 35 added; `server.rs` 151 to 152 says "An earlier version closed the connection silently instead". CLAUDE.md: "history lives in git and the commit messages".

### F10. The collector's record (both; modes B only)

`speech.rs` 457 to 466 hold `texts`, `traces`, `endings`, `unended`, `started`, `timings` and `order`, all by `UtteranceId`; `Timing` (272) holds the text again; the reader thread keeps a further `texts` map (1914). A sound (632 to 642) gets `texts`, `started` and `endings` but no `timings`. `collect_only: bool` (481) and `compare: Option<Comparison>` (487) can both be set; 16 lines read `self.compare` (B said 32 sites). Fix: one record per utterance with a state enum, and one mode enum.

### F11. Frames read three times (B only; confirmed)

`absorb` (590 to 657), `record(timeline, &mut texts, &frame)` (1934) with its own text map, and `xtask/src/nvda.rs` `Source::Verbatim` with a third, `texts.remove(&utterance.0).unwrap_or_default()` (503), which also prints an empty text for an ending whose utterance was not seen.

### F12. The overlapped pipe twice (B only; confirmed as a copy, size not recounted)

`tunnel.rs` 12 to 21 says the handle is opened with `FILE_FLAG_OVERLAPPED` and "each direction uses its own event, exactly as the server does on its side of the same pipe", and 168 to 172 names "the same documented-safe pattern `verbatim_control::server`'s `RawPipe` uses" (`server.rs` 394, `read` 499, `write_all` 536). The tunnel's copy adds a stop event the server's lacks, so the shared part is less than B's 150 lines.

### F13. Dead code (both)

`grep -rn "take_collected"` over `crates` and `xtask`: only its definition (`speech.rs` 1474). `grep -rn "\.utterances()"`: only four tests in `timeline.rs` (562, 573, 585, 638); its doc ("the substring of the timeline that SpeechCollector matches its matchers against") is false.

### F14. Stale documents (both)

`testing.md` 52 says File Explorer's placeholder is accepted "at most once in all (Dickson, 2026-10-21)"; `scenarios/explorer_folder_window.rs` 99 to 127 accepts `optional_up_to(placeholder, 2)` and asserts `said <= 2`, citing "Dickson, 2026-11-19". The standard should state the later ruling. The add-on's docstring (7) cites docs/nvda-transcript.md, which does not exist; the guide is `books/dev/src/how-to/nvda.md`.

### F15. Dispatch fallbacks (A only; confirmed)

`server.rs` 415: `unreachable!("OpenControlTunnel is handled in handle_connection before dispatch")`; 437 `"not a launch request"`; 593 `"not a process request: {other:?}"`; 685 state; 913 desktop; 951 scenario windows. Each group is matched in `dispatch` and again in its handler. CLAUDE.md: "Code reaches no fallback that could only be a logic bug". Fix: nested request enums or destructured arguments.

### F16. Two maps per launch (A only; the merge confirmed, the proposed release not)

`process.rs` 595 `static LAUNCHED: Mutex<BTreeMap<u32, Launched>>`, `jobs.rs` 54 `static JOBS: Mutex<BTreeMap<u32, JobRecord>>`, both keyed by pid; `jobs::forget` runs from `launched.retain` at the next launch once `FINISHED_KEPT = Duration::from_mins(5)` has passed (224 to 236, 592). The job record can live in `Launched`. A's release at connection end covers only launches already finished then; one still running when its connection ends still needs a release, so the sweep is not shown removable.

### F17. Held keys in a global (B only; confirmed)

`server.rs` 55 `static HELD: Mutex<Vec<(u64, String, KeyName)>>`, filtered by connection in `release_held` (64 to 70) and `key_transition` (92 to 94), released from the connection's drop. It can be the connection's own field.

### F18. A failed image read (A only; confirmed)

`desktop.rs` 538 `image: image_name(pid).unwrap_or_default(),`; `terminals.rs` 126 `if !info.image.eq_ignore_ascii_case(IMAGE) || info.class != CLASS { return; }`. A Windows Terminal window whose process cannot be opened is never recorded as shown, so the guard on the owner's own terminal misses it.

### F19. Trivial tests (A only; confirmed)

`scenario_dir_joins_root_and_name`, `resolve_artifacts_root_honors_the_override`, `resolve_artifacts_root_defaults_under_the_workspace_root` (`artifacts.rs` 864 to 888) restate one-line functions.

### F20. Latency in two places (B only; confirmed)

The collector's timings go to `latency.csv` (`scenario.rs` 71, 2723), which no code reads; `cargo xtask measure` reads Verbatim's `verbatim::latency` log lines (`xtask/src/perf.rs` 365). `testing.md` 64: "Response times are asserted... (Dickson, 2026-10-08, reaffirmed 2026-10-09). This is decided but not yet built: today the suite records the latency of each utterance and asserts none (`SpeechCollector::latency_rows`...)". See section 4.

### F21. The optional matcher at M5 (B only; confirmed)

Uses: `terminal.rs` 747 to 748 and 795 to 796, `explorer_folder_window.rs` 115, `settings_search_suggestions_reentered.rs` 107. `testing.md` 53 ends "the rule for the duplicate focus comes with Windows Terminal's app module in M5". Line estimates are B's.

## 3. Test findings

`verbatim-agent` (64 tests, 1,937 lines):

- Removable: the 9 tests of `protocol.rs` (1580 to 1951, 372 lines) round-trip serde derives of types both ends compile from, one of them the dead default (F9); keep one wire-format test: about 330 lines.
- Replaceable: `server.rs`'s 364 test lines carry their own small client; after F3 the generic call serves both, about 60 lines.
- To add: an `accFocus` failure is an error (F4); a lost duplication or size change fails the capture (F5): about 40 lines.

`verbatim-e2e`, this area's files (104 tests, 2,459 lines):

- Removable: `every_defect_message_is_found` (F1, replaced by a 10-line marker test); the four `Timeline::utterances` tests (40); the three path tests (25).
- Replaceable: the summary write and parse tests (about 120 lines in 5 to 6 tests) by one serde round trip (F8); `nvda.rs`'s 8 poller tests (287 lines) by about 120 lines of push-reader tests (F6).
- Shrinkable at M5: about 250 lines of optional-utterance tests (F21).
- Keep: `speech.rs`'s 42 tests, which run the real collector on frames in the server's order, the cheapest level that can fail.
- To add: one test that the add-on's reply shape parses or fails (F6).

Outside this area's files but raised: the registry agreement test (45 lines, F2); B's note that 18 scenarios are `local_only: true` (confirmed: `grep -rh "local_only: true" crates/verbatim-e2e/src | wc -l` gives 18; `registry.rs` 156 says "Every local and VM run includes it", and GitHub's `ci.yml` 481 sets `VERBATIM_E2E_SKIP_LOCAL_ONLY: "1"`), which belongs to the scenario pair's review.

`xtask nvda` and the add-on: only the committed-package test (`xtask/src/nvda.rs` tests) and `runs_pinned_build`'s test.

## 4. Questions for Dickson

### Captures cited by expectations are not kept, and nothing checks an expectation against its capture

The facts, settled:

- `.gitignore` line 1 is `/target`. Captures are cited under `target/e2e-evidence/`.
- Citations in Rust sources (`grep -rnoE "e2e-evidence/[A-Za-z0-9._/-]+" --include=*.rs crates xtask`): 25 places in 13 files, 17 distinct paths, 13 folders. Counting the dated folder names without the prefix adds 9 file-level citations in `password.rs` (257, 326) and `selection_keys.rs` (107, 145, 165, 270, 291, 303, 311), all inside the same folders. With `books/` and `docs/` (10 parity, NVDA and design files), 16 folders cited from 23 files. B's "16 folders" is right for that wider count; A's "13 distinct paths" is the Rust count of folders, not paths.
- Each Rust citation, with what exists. Present in the working tree: only `rtca-caret-keys-2026-11-17/` (`caret_keys.rs` 23; `cmp-notepad_caret_keys.txt` and `cmp-text_box_caret_keys.txt` exist) and `rtca-selection-keys-2026-11-17/` (`selection_keys.rs` 27; both `cmp-*` files exist). In git: none at HEAD (`git ls-files | grep -c e2e-evidence` is 0); the two caret-keys files were committed by 66a15d85 and removed by c341f768 ("Keep the caret-key captures out of the repository, as target is", 2026-11-17), so they are in history. Absent from the tree and from all of git history (checked by file name across `git log --all --name-only`):
  - `m42-text-captures-2026-10-23/`: `caret_keys.rs` 12, `document_keys.rs` 14, `password.rs` 257 (`20a-forms-password-type-a-run1.txt`), 263, 326 (`20b-wpf-password-type-a-run1.txt`), `selection_keys.rs` 23, 107, 165, 291, 303, 311, `typed_words.rs` 151.
  - `m42-desktop-2-2026-10-22/`: `editing.rs` 125 (`part3b-textbox-1-type-over.txt`, `part3b-textbox-2-cut.txt`), 215 (`part3b-notepad-1.txt`, `part3b-notepad-2.txt`), `selection_keys.rs` 145, `settings_search_suggestions_reentered.rs` 99 (`part3a-nvda-2.log`), `terminal_windows.rs` 46, `mockapp/tests/caret_watch.rs` 860, 909.
  - `m42-selection-keys-2026-10-22/`: `password.rs` 15, `selection_keys.rs` 16, `caret_watch.rs` 1055 (`textbox-word-delete.txt`), `verbatim-core/tests/text.rs` 1206.
  - `m42-desktop-4-2026-10-22/notepad-cut-after-typing-over.txt`: `editing.rs` 228, `selection_keys.rs` 270.
  - `m42-word-at-end-2026-10-22/text-box-editing-compare.txt`: `editing.rs` 140.
  - `m42-protected-fixes-2026-10-22/`: `password.rs` 18.
  - `m42-selection-fixes-2026-10-22/notepad-first-word-backspace.txt`: `selection_keys.rs` 20.
  - `fixes-round-eight-2026-10-21/nvda-expanded-combo-box-capture.txt`: `settings_combo_box_expanded.rs` 5.
  - `fixes-round-six-2026-10-21/rapid-tab-nvda-1.txt`: `settings_search_suggestions_reentered.rs` 6.
  - `switcher-pane-2026-10-24/NvdaSw/`: `verbatim-core/tests/reduce.rs` 3186.
  - `listener-no-connections/proxy-rule-descriptions.txt`: `verbatim-uia/src/answered.rs` 144 (a recording of descriptions, not an NVDA capture).
- By contrast, four mockapp tests cite evidence under `experiments/` that git tracks (`call_counts.rs` 2422, 2610; `focus_reports.rs` 1989, 3162).
- What the rules say about where a capture is kept:
  - CLAUDE.md: "take spoken expectations only from captures"; and "Studies, reviews, design drafts, run logs and measurement records are kept in the session's scratchpad, not the repository. Something enters `experiments/` only when a lasting document cites it as the evidence for a decision still in force, and only the part it cites; a new file in `experiments/` or `docs/design/` is proposed to Dickson before it is committed."
  - `books/dev/src/how-to/nvda.md` 180 to 182: "Captures are working material, not kept in the repository: they serve exploratory testing, comparing what Verbatim says with what NVDA says, and writing the end-to-end and unit tests that pin the behavior down."
  - `books/dev/src/reference/testing.md` says nothing about where a capture is kept; its only mention is line 11, on compare mode: "a research instrument, not a test: it asserts nothing and never runs in CI".
- Nothing checks an expectation against a capture: no test or CI step reads a capture; compare mode, run by hand, is the only mechanism that sets a scenario's expectations beside NVDA's speech.

The question: keep each scenario's cited NVDA capture as a small committed file (which needs Dickson's word under the `experiments/` rule), or cite only re-runnable compare reports and drop the dead paths? Speech: none changes.

### Compare mode: keep it, schedule it, or retire it for `cargo xtask nvda capture`

Laid out without recommending:

- What it does: `compare.rs` 1 to 51. With `VERBATIM_E2E_COMPARE` set to a scenario's name, that scenario's own setup and body run against NVDA instead of Verbatim, reading NVDA's speech from the transcript add-on; each assertion is recorded against its step, not checked; a report (`compare.txt`) gives each step's expected texts beside NVDA's, judged on normalized text (utterance boundaries, sounds, cancels and spacing dropped) as same, differs, declared deviation, declared-but-agrees or not compared. It asserts nothing.
- What runs it: `cargo xtask nvda compare <scenario>` (`xtask/src/nvda.rs` 590 to 640), which first refuses an NVDA that is not the pinned build, then runs `cargo test -p verbatim-e2e --test e2e -- --ignored --exact <name>` with the variable set. No workflow under `.github/workflows` sets it. The guide is `books/dev/src/how-to/nvda.md` ("Comparing a scenario"). `testing.md` 11 and `compare.rs` 6 say it never runs in CI.
- When it last changed: `compare.rs`'s last code change was 39a6d648 (2026-10-22); later commits there (e7850876, a7c954e9) moved document paths. It was last used for committed work on 2026-11-17: `selection_keys.rs` 25 to 30 and `caret_keys.rs` 23 cite `cmp-*` reports in `rtca-*-2026-11-17/`, and d47d89a6 and 66a15d85 assert their speech.
- Lines: `compare.rs` 492 product and 324 test; it also reaches into `speech.rs` (70 lines mention `compare`), `scenario.rs` and `registry.rs` (`Comparable` on each definition, 48 `Comparable::No` in `crates/verbatim-e2e/src`), and 29 `deviates_from_nvda(` calls in scenarios feed only its verdicts. The reviewers' total is 1,450 to 1,500 product and 600 to 650 test lines.
- Planned work that names it: `docs/roadmap.md` 657 to 658, M4.2's exit: "each entry is matched (verified) by a scenario, with a declared deviation where Dickson chose one, checked with compare mode." Two open follow-ups from M4.1 concern it (553, 574 to 577). Retiring it changes that roadmap entry; no fact since it was written is named by either reviewer.
- How it relates to `cargo xtask nvda capture`: both live in `xtask/src/nvda.rs`, start or reach the same agent, use the same client (`verbatim_e2e::nvda`: `Transcript`, `Settling`, `QuietWait`), and refuse an unpinned NVDA through the same `require_pinned_nvda`. `capture` drives keys, launches and bring-forwards given on its command line and prints what NVDA queued after each (or, with `--verbatim`, what the local Verbatim queued), as text or JSON; it knows nothing of scenarios or their expectations. `compare` runs a scenario's own steps and sets its expectations beside NVDA's speech.

### Latency budgets: one source

When the decided budgets (`testing.md` 64, Dickson, 2026-10-08, reaffirmed 2026-10-09) are built, should they read the collector's per-utterance rows (`latency_rows`, written to `latency.csv`, which nothing reads), or Verbatim's `verbatim::latency` log lines that `cargo xtask measure` reads? The fact that changed since the decision named `latency_rows`: the measure set took its latency from the log. No speech changes.

## 5. Cost of one behaviour

The usual new behaviour here is one more independent read or wait through the agent. Checked with `git show --stat`:

- f5a4c4df (WaitForUiaAnswer, with the defect gate), 10 files, +375 −28: two crate guides and `testing.md`, `desktop.rs`, `protocol.rs`, `server.rs`, `agent_client.rs`, `artifacts.rs`, `scenario.rs`, a scenario.
- e62596e9 (WaitForLastShownLine), 7 files, +289 −31: the agent guide, `focus.rs`, `protocol.rs`, `server.rs`, `agent_client.rs`, `scenario.rs`, a scenario.
- 86edcf16 (SwitcherItems and launch diagnostics), 14 files, +985 −186.
- A's third sample, 88541265, is a 53-file product change (a text focus's caret read) and does not measure this area.

Each request touches: a `Request` and a `ReplyPayload` variant, a changelog paragraph and the version (`protocol.rs`); an arm in `dispatch` and one in a sub-dispatcher with its fallback (`server.rs`); the handler; a client method with its `unexpected` arm; a `Scenario` wrapper; one or two guides. A new scenario also touches `tests/e2e.rs`.

Shared mechanisms, merged: typed requests with an associated reply type and one generic call (F3) remove the client method and make a wrong reply unrepresentable; nested request enums (F15) remove the second arm; trials built from the registry (F2) remove the `tests/e2e.rs` edit; history left to git (F9) removes the changelog paragraph. That leaves the request type, its handler, the `Scenario` call and the guide line.

## 6. Dropped

- A, F3: "`request()` silently drops any frame whose id is not the request's... it should fail". Contradicted: a request that times out on the read (`request_waiting`, `agent_client.rs` 148 to 165) returns an error while its reply can still arrive, and the next request on the same client reads it; skipping by id is that path's handling.
- B, F17: "the guides restate the code... the crate guide... about half its length". The line counts are right (`verbatim-e2e.md` 987, `e2e.md` 984, `verbatim-agent.md` 404), but the restatement and the halving were not checked line by line in this merge.
- B, F13 in part: calendar arithmetic as part of the summary format (it serves history folder names and log timestamps; noted in F8).
- Counts corrected rather than dropped: DEFECT_MESSAGES has 23 strings (B: 24); `unexpected` arms 55 (A: 56); scenario names 126 (A: 129); agent tests 64 (A: 57); `self.compare` 16 lines (B: 32); A's capture count of "13 distinct paths in 15 places" (settled in section 4).
