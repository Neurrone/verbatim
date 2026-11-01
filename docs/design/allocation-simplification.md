# Allocation simplification

Status: designed on 2026-11-01 from Dickson's decisions of that day, to be built as one build. It removes code whose only purpose is to avoid an allocation, a copy or a local call, and the entries and figures in the performance ledger (`docs/performance.md`) that record costs of that size. Each item was checked against the code at 0da2333c; the audit that found them ran at 101e32b9.

## The build

A rule for every item: the code becomes the plain form, its behaviour stays the same, and only the figures named below change. Nothing in this build adds a call into another process, a wake-up or a timer.

### The flight recorder's size estimate

- Today: `RecordedInput::estimated_bytes` (`crates/verbatim-core/src/recorder.rs`, lines 27 to 293) walks the input with `HeapCounter`, a hand-written serde `Serializer` with its own error type `NeverFails` and the seven compound `Serialize*` impls, which adds each string's length and each element's inline size, so that the estimate writes and allocates nothing.
- Replacement: a counting writer of a few lines, a struct holding a `usize` whose `io::Write::write` adds `buf.len()` and returns it, fed by `serde_json::to_writer`. The estimate is the length of the entry's compact JSON. `verbatim-core` already depends on `serde_json`.
- What changes: the 8 MiB bound (`ReducerRecorder::DEFAULT_MAX_BYTES`) counts JSON length, which for text is the text plus quotes and escapes, and for everything else field names and digits rather than inline sizes. The dumps of `measure_keys_notepad` on 2026-11-01 held 1,004 inputs in 343 KB of input JSON, so the entry bound still governs ordinary use; a whole-document terminal read of about 1.1 MB counts a little more than before, so the bound holds slightly fewer of them. The JSON is formatted on the reducer thread for every input: the build measures, in a release build, the recording of a whole-document read before and after, and stops and reports to Dickson if it adds more than 0.5 ms, a quarter of the reducer's 2 ms budget for a step.
- Tests: `an_inputs_text_is_counted_in_its_estimate` asserts that an input with 100,000 ASCII characters of text estimates exactly 100,000 more than the same input with empty text, which needs no copy of the production logic; `inputs_carrying_text_are_bounded_by_bytes` stays as it is.

### A large frame's hand-over in the relay

- Today: `PortReader::take_bodies` (`crates/verbatim-process/src/transport.rs`, lines 560 to 607, 659 to 670 and 740 to 746) hands out a `FrameBody`, either `Borrowed` parts or `Owned`, the reader's own buffer given up when a frame gathered over several reads fills it exactly, so that the relay (`crates/verbatim-supervisor/src/owner.rs`, lines 527 to 644) keeps a large frame without copying it.
- Replacement: `FrameBody`, `take_bodies` and `is_owned` go. `PortReader::take` always lends `Parts`; `hand_out` passes every frame through `whole_frames`, keeping its rule that a large frame's buffer is given back once empty. `Parts` gains `into_owned()`, the copy `FrameBody::into_owned` makes today for a lent frame, and the relay calls it where it calls `body.into_owned()` now; `outpost_frame` and `spare_frame` take `Parts`. `OwnedFrame` loses its `start` field, which is then always 0.
- What changes: a frame larger than one read is copied once more in verbatim.exe, about 0.1 ms a megabyte (the ledger's figure for copying text, "A terminal's rotation"), on the owner thread, once per whole-document read.
- Tests: the transport tests drop the given-up flag: `a_port_reader_gives_up_its_buffer_for_a_large_frame_read_whole` becomes `a_port_reader_hands_out_a_large_frame_read_whole`, asserting the frame whole and as written; `a_port_reader_lends_a_frame_followed_by_the_start_of_another` and `a_port_reader_hands_out_a_frames_raw_section_apart_from_its_body` assert the same frames without the flag; the helpers `turn_bodies` and `turn_parts` fold into `turn`.

### The terminal conversion's spare buffers

- Today: `crates/verbatim-outpost/src/terminal.rs`, lines 248 to 290, keeps a pool (`SPARE`, at most `SPARES`, two, pairs of buffers, each shrunk to `SPARE_KEPT_BYTES`, 64 KiB), which the outbound writer refills after writing a read (`terminal::recycle`, `crates/verbatim-outpost/src/outpost/outbound.rs`, lines 242 to 245, through `terminal_read`, line 338), so that an ordinary read allocates nothing for its text and screen.
- Replacement: the pool, `recycle`, `spare`, `SPARES`, `SPARE_KEPT_BYTES` and the outbound `terminal_read` go. `unpadded_utf16(units)` keeps its one pass from UTF-16 to unpadded UTF-8 and returns a new `TerminalText`.
- What changes: each read allocates its two buffers; a whole-document read already did, since the pool kept only 64 KiB.
- Tests: `converted_alike` and its two callers (`a_reads_text_converted_in_one_pass_is_the_two_copies_exactly`, `unpaired_surrogates_and_line_ends_convert_as_the_two_copies`) convert without a reused buffer; the ignored `measure_conversion` measures the one pass into new buffers as "after".

### Core's small checkpoint

- Today: `SrState`'s doc (`crates/verbatim-core/src/state.rs`, lines 645 to 650) requires anything that grows to be held behind `Arc` so that a checkpoint clone is small; `tests/alloc.rs` pins it with `a_checkpoint_with_text_allocates_a_small_fixed_amount`, `a_checkpoint_allocates_a_small_fixed_amount_whatever_the_ancestor_chain` and `the_themes_fetches_change_the_state_without_allocating` (lines 345 to 387); four serde helpers (`serialize_shared`, `deserialize_shared`, `serialize_chunk`, `deserialize_chunk`, lines 1044 to 1067) serialize the `Arc` fields.
- Replacement: the three tests and the rule go; the doc says only what the state is and that the flight recorder clones it at a checkpoint, which happens once per segment of its window. The helpers give way to serde's `rc` feature on the workspace's `serde` dependency, which serializes an `Arc<T>` as its `T`, so the dump format does not change; `cargo hakari generate` then updates `workspace-hack`. Existing `Arc` fields stay.
- Kept: the per-step tests of `tests/alloc.rs` (a focus step, a navigation step, the text steps and a terminal line each allocate the same whatever the ancestor chain), because a step whose cost grows with the application is a real cost. The module doc loses its checkpoint sentence.

### The hook's window-class cache

- Today: `crates/verbatim-ia2/src/hook.rs` keeps each window's class in a thread-local `CLASSES` (line 350), a `WindowCache<Rc<str>>` set up for a hook set that hears windows destroyed (lines 556 to 567) and consulted by `wanted_kind` (lines 716 to 745), so a flood of events from one window reads its class once.
- Replacement: `CLASSES` and its set-up and clearing go; `wanted_kind` loses its `classes` argument and keeps its closure that reads the class at most once per event, now with `class_of(hwnd)` into an `Option<String>`, returning a `String` where it returns an `Rc<str>`. `WindowCache` stays for the arbitration (`crates/verbatim-outpost/src/arbitration.rs`), where it saves a cross-process probe.
- What changes: an event NVDA's filters ask the class of (child objects for the Excel check, a foreground change, a menu event, a tooltip's show) reads it with one `GetClassNameW`, a local call. The console host's events never ask it.
- Tests: `a_windows_class_is_read_once_for_its_events` and `a_destroyed_windows_class_is_forgotten` go; `the_listener_reads_a_foreground_windows_class_once` stays as the once-per-event check; the other tests built on `outpost_kinds` drop the `classes` argument.

### Core's caret history

- Today: `SrState::caret_history` (`crates/verbatim-core/src/state.rs`, line 689) is `[Option<TimedCaret>; CARET_HISTORY]`, rotated in place (`crates/verbatim-core/src/editing.rs`, lines 1255 to 1259 and 1314), so that a checkpoint copies it without allocating.
- Replacement: a `VecDeque<TimedCaret>` kept to `CARET_HISTORY`, 8: a report is pushed at the back and the front popped past 8; emptying is `clear`; `caret_before` iterates it without `flatten`. `caret_moved_by_core` pushes its time-zero record after `update_caret` emptied the history, as it writes the last slot now.
- What changes: the dump's state line holds the reports as a list rather than eight slots, so `DUMP_FORMAT_VERSION` becomes 6 ("the caret history is a list of the reports it holds") and `tests/fixtures/focus_value_states.jsonl` is regenerated with its ignored `regenerate_fixture`.
- Tests: unchanged but for the fixture.

### The latency ledger's last key

- Today: `LatencyLedger::last_key` (`crates/verbatim-app/src/latency.rs`, lines 483 to 487, 535 to 538 and 716 to 717) packs a time and a flag into one `AtomicU64` so that the hook thread stores both without a lock.
- Decided replacement: a plain field under the ledger's lock, or a `Mutex<Option<(u64, bool)>>`.
- Open: `key_seen` runs on the keyboard hook's thread (`key_reports::reports`, `crates/verbatim-app/src/key_reports.rs`, line 209), and `docs/design/ownership.md`, "Who may block on whom", rules that the hook takes no lock, because the hook's `KEY_ORIGINS` lock was a priority inversion; the ownership check covers only the hook's own modules, so it would not catch this. A separate `Mutex` would be contended only by the speech queue thread's read, but it is still a lock on the hook thread. Recommendation: keep the packed atomic and drop this item. The build does this item last, in a commit of its own, only with Dickson's answer.

### Order of work

1. Core: the recorder's counting writer, the checkpoint rule and its tests with serde's `rc` feature, and the caret history with the dump version and fixture.
2. The process, supervisor, outpost and hook crates: the relay's lending, the terminal conversion without a pool, and the hook without its class cache, with the crate guides each changes (`docs/crates/verbatim-core.md`, "The flight recorder" and "The allocation invariant"; `verbatim-process.md`; `verbatim-supervisor.md`; `verbatim-outpost.md`; `verbatim-ia2.md`).
3. Documents: the ledger's removals below, the outpost conversion's figures restated, and the instruction changes in this repository's files below.
4. The latency ledger's last key, only if Dickson keeps that item.

`cargo xtask ci` runs once at the end. Wake-ups and processor time are measured before and after inside the build with `measure_keys_conhost` and `measure_keys_notepad` (debug, test audio mode), with the release measurements named under each item.

### Scenarios

Run three times each at the end of the build: `conhost_full_history_flood` (whole-document frames through the relay, the conversion without a pool, the recorder's JSON count of large reads), `notepad_caret_keys` (Core's caret history judging the caret before a key) and `notepad_and_verbatim_menu` (the hook's class reads for menu and foreground events). `conhost_full_screen` and `conhost_scrollback_overflow` would also cover large reads but are recorded in `docs/flaky-tests.md`, so they are left out.

## The ledger's removals

The ledger keeps what is substantially slow: calls into other processes, wake-ups, waits, timers, and work that grows with the application or reaches milliseconds. Removed, by entry:

- "A focus change, UIA, steady state": the local work in the bullet beside the calls (the `GetAncestor`, the uncontended lock, the readers' lock, the two or three timer calls); the bullet keeps its watchdog wake-ups, moved-on calls and delays.
- "A WinEvent focus no UIA focus answers, and the gap's read": the first two bullets (per-event `GetAncestor`, lock takes, `SetThreadpoolTimer` and the restamp) and the memory bullet.
- "The listener's reads in a window the outpost judges MSAA": "its message about 10 bytes larger for its answer".
- "A focus below a hung window, UIA": the bullet on the cost on the steady path.
- "A focus whose walk is stuck at its bound, the outpost": the lock takes in the steady-path bullet, and the bullet on copies per focus.
- "The outpost's reducer": the bullets on processor time, locks and memory.
- "The outpost's object store": the whole entry, which adds no call or wake-up.
- "A key through the keyboard hook": "Neither waits or allocates", "one boxed packet each", and the scan of 1,024 trace pairs with its 16 bytes and 16 KiB.
- "Speech only the reducer makes": the speech queue's list of marks and table insert, and the bullet on a synthesizer chunk's decode and encode in microseconds.
- "A caret report, UIA": the bullet beside the calls.
- "Ordering focuses across processes": the clock read and the message size changes in bytes; what it does not catch stays.
- "One judge: reading for every node the outpost has not released": the bullet on `Outbound::waiting_nodes`.
- "The task switcher's frame": the whole entry, which adds no call.
- "The task switcher's items, and a grid item's row and column": "Every snapshot is 40 bytes larger."
- "A window's handle in node details": the whole entry.
- "Core's meeting of chains": the whole entry, which adds no call and records only message sizes, comparisons and allocations.
- "A selection inside a list the focus controls, UIA": the comparison of runtime ids and "no lock".
- "A caret report, Win32 edit control": the anchors' lock per value change.
- "A terminal output line": the sentence on the selection and the anchors' lock.
- "A terminal flood": the flood policy's walk of its queue.
- "A terminal's rotation": the bullet on freeing the steps afterwards.
- "A full-screen program's exit": the paragraph on the aside screen's hashes.
- "What Core last spoke of the focus": the whole entry.
- "A terminal's reads in Core": the kept buffers' bytes in the paragraph on the outpost's conversion, which is restated for the conversion without a pool, with its whole-document time kept.
- "A terminal review command above the last read": the hash comparisons' few microseconds.
- "A terminal's command marks": the hash comparison's few microseconds.
- "The terminal memory's cost, estimated": the paragraph of sizes per type on x64, and the paragraph on the vote's and the caret line check's microseconds; the spare buffers and the flight recorder's bound restated for this build.
- "The listener's owed answer per UIA focus event": the whole entry.

## The cause

Every instruction that set the project's cost rules named allocations, copies, locks and local calls beside cross-process calls and wake-ups, with no threshold, so each session treated a single allocation as a cost to avoid in code and to record in the ledger. Traced:

- The port skill's review reference (`.claude/skills/port/references/review.md`, from b4391ae5, 2026-10-15) lists "an allocation, copy, or parse on the per-event path that could be done once" as a cost finding, and the skill's costing step (`SKILL.md`) asks for what a change "adds to the per-event work".
- `REVIEW.md` from a9b3e727 (2026-10-21) until 331ca0dc (2026-11-01) failed a change for "a call, wake-up, lock, timer or allocation added on the event or speech path that the ledger does not state", so builders recorded each allocation, lock and byte in the ledger to pass review: the window handle's 16 bytes (10c9e83a), the trace pairs' 16 KiB (8be86807), the microsecond stamps' 31 bytes (697ab4f4), what Core last spoke (d5068b48) and the listener's owed answer, written as a review fix (fb83a51a).
- The communication design's per-event cost step (step 11, 2026-10-13 and 14) gave the class cache (3f0b8002), the recorder's serializer (610f7775) and the relay's given-up buffer (4afba986); the terminal's spare pool (4de51afa) grew from Dickson's request for one pass.
- The phase 6 design's allocation invariant (b8ea103d, 2026-10-06) asked that a step's allocation not grow with the state, which is kept; its test (41cb27f6) also bounded a checkpoint's absolute size, which became `SrState`'s rule and the inline caret history (64a69b0c).
- `CLAUDE.md`'s rule to judge every change on wake-ups and CPU reads as covering any processor work, however small.

No memory file is responsible: `worst-case-performance.md` concerns worst cases and remote programs' instruction counts.

### Proposed instruction changes

For Dickson, not made in this build: `CLAUDE.md`'s standing rule "Judge every change on wake-ups and CPU as well as latency, and raise neither on the event or speech path unless that is the change's purpose, because a latency figure alone hides a cost paid on every event." becomes "Judge every change on wake-ups, calls into other processes and measured processor time as well as latency, and raise none on the event or speech path unless that is the change's purpose, because a latency figure alone hides a cost paid on every event; an allocation, a copy, an uncontended lock or a local call is not a cost to design around or to record unless a measurement shows it matters, because code shaped to avoid one adds complexity for nothing."

In this build's documents commit, pending Dickson's acceptance:

- `.claude/skills/port/references/review.md`: "an allocation, copy, or parse on the per-event path that could be done once;" becomes "work on the per-event path that grows with the application (a document, a history, a tree) and could be done once, or that a measurement shows costs;".
- `.claude/skills/port/SKILL.md`, the costing step: "and what it adds to the per-event work and wake-ups of the listener, the outpost, or Core" becomes "and what it adds to the wake-ups of the listener, the outpost, or Core, and to per-event work that grows with the application".
- `docs/performance.md`, after its first paragraph: "The ledger records what is substantially slow: calls into other processes, wake-ups, waits and timers, and work that grows with the application or takes milliseconds. A single allocation, a type's size, an uncontended lock or a local call of microseconds is not recorded."
- `docs/crates/verbatim-core.md`, "The allocation invariant": keeps the per-step assertions and drops the checkpoint's bound and the rule that whatever grows is held behind `Arc`.

`REVIEW.md` already says this since 331ca0dc and needs no change.

## Limitations

- Dumps written before the build no longer load, by the dump format's version check, because of the caret history's shape.
- The caret and selection rework (`docs/design/caret-selection-rework.md`) removes `caret_history` altogether, so the caret history's change lasts only until that rework, which is ordered later.

## Decisions

Dickson, 2026-11-01:

- Simplify the flight recorder's size estimate, the relay's large-frame hand-over, the terminal conversion's spare buffers, Core's small-checkpoint rule, the hook's window-class cache, Core's caret history and the latency ledger's last key, as above; the last is open on the hook's no-lock rule.
- Keep `ForegroundView`'s `ArcSwap`.
- Remove every inconsequential entry or figure from the ledger, which is for substantially slow things such as cross-process accessibility calls.
- Find the cause of those entries and propose the wording of each instruction responsible, with changes to `CLAUDE.md` and memory files as proposals.
