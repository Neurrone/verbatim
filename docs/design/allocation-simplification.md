# Allocation simplification

Status: designed on 2026-11-01 from Dickson's decisions of that day, revised after its adversarial review, to be built as one build. It removes code whose only purpose is to avoid an allocation, a copy or a local call, and the entries and figures in the performance ledger (`docs/performance.md`) that record costs of that size. Each item was checked against the code at 0da2333c; the audit that found them ran at 101e32b9.

## The build

A rule for every item: the code becomes the plain form, its behaviour stays the same, and only the figures named below change. Nothing in this build adds a call into another process, a wake-up, a timer or a lock.

### The flight recorder's size estimate

- Today: `RecordedInput::estimated_bytes` (`crates/verbatim-core/src/recorder.rs`, lines 27 to 293) walks the input with `HeapCounter`, a hand-written serde `Serializer` with its own error type `NeverFails` and the seven compound `Serialize*` impls, which adds each string's length and each element's inline size: constant work per string, whatever its length.
- Replacement, if the gate below allows it: a counting writer of a few lines, a struct holding a `usize` whose `io::Write::write` adds `buf.len()` and returns it, fed by `serde_json::to_writer`. The estimate is the length of the entry's compact JSON. The `Result` is ignored as today, the count so far standing, because the writer cannot fail and every input serializes, which the dump relies on too.
- The gate: `record_input` runs on the reducer thread under the recorder's lock, after `reduce` and before the step's effects run (`crates/verbatim-app/src/main.rs`, lines 1603 to 1611), so formatting the JSON, which scans every byte of text and escapes each line's `\r\n`, delays the step's speech. Whole-document steps already sit at the reducer's 2 ms budget (`docs/performance.md`, "A terminal's reads in Core") and a 4.0 MB document takes 4.9 ms ("A terminal's rotation"). The build first measures, in a release build, the JSON count of the worst cases, the 4.0 MB document and a whole document of Cyrillic text, against `HeapCounter` on the same inputs. If the JSON count adds more than 0.1 ms on the 4.0 MB document, this item is not built: `HeapCounter` stays, recorded in its doc comment as kept because its cost is constant per string, not to avoid an allocation.
- What changes if built: the 8 MiB bound (`ReducerRecorder::DEFAULT_MAX_BYTES`) counts JSON length, which for text is the text plus quotes and escapes, and for everything else field names and digits rather than inline sizes. The dumps of `measure_keys_notepad` on 2026-11-01 held 1,004 inputs in 343 KB of input JSON, so the entry bound still governs ordinary use; a whole-document read counts a little more than before, so the bound holds slightly fewer of them.
- Tests if built: `an_inputs_text_is_counted_in_its_estimate` asserts that an input with 100,000 ASCII characters of text estimates exactly 100,000 more than the same input with empty text, which needs no copy of the production logic; `inputs_carrying_text_are_bounded_by_bytes` stays as it is.

### A large frame's hand-over in the relay

- Today: `PortReader::take_bodies` (`crates/verbatim-process/src/transport.rs`, lines 560 to 607, 659 to 670 and 740 to 746) hands out a `FrameBody`, either `Borrowed` parts or `Owned`, the reader's own buffer given up when a frame gathered over several reads fills it exactly, so that the relay (`crates/verbatim-supervisor/src/owner.rs`, lines 527 to 644) keeps a large frame without copying it.
- Replacement: `FrameBody`, `take_bodies` and `is_owned` go. `PortReader::take` always lends `Parts`; `hand_out` passes every frame through `whole_frames`, keeping its rule that a large frame's buffer is given back once empty. `Parts` gains `into_owned()`, the copy `FrameBody::into_owned` makes today for a lent frame, and the relay calls it where it calls `body.into_owned()` now; `outpost_frame` and `spare_frame` take `Parts`. `OwnedFrame` loses its `start` field, which is then always 0.
- What changes: a frame larger than one read is copied once more in verbatim.exe, on the owner thread, once per whole-document read: about 0.2 ms a megabyte, so about 0.9 ms for the 4.0 MB document. verbatim.exe's peak memory rises by the frame's size while the copy and the reader's buffer are both held, until the reader gives its buffer back. The build measures both in a release build.
- Tests: the transport tests drop the given-up flag: `a_port_reader_gives_up_its_buffer_for_a_large_frame_read_whole` becomes `a_port_reader_hands_out_a_large_frame_read_whole`, asserting the frame whole and as written; `a_port_reader_lends_a_frame_followed_by_the_start_of_another` and `a_port_reader_hands_out_a_frames_raw_section_apart_from_its_body` assert the same frames without the flag; the helpers `turn_bodies` and `turn_parts` fold into `turn`.

### The terminal conversion's spare buffers

- Today: `crates/verbatim-outpost/src/terminal.rs`, lines 248 to 290, keeps a pool (`SPARE`, at most `SPARES`, two, pairs of buffers, each shrunk to `SPARE_KEPT_BYTES`, 64 KiB), which the outbound writer refills after writing a read (`terminal::recycle`, `crates/verbatim-outpost/src/outpost/outbound.rs`, lines 242 to 245, through `terminal_read`, line 338), so that an ordinary read allocates nothing for its text and screen.
- Replacement: the pool, `recycle`, `spare`, `SPARES`, `SPARE_KEPT_BYTES` and the outbound `terminal_read` go. `unpadded_utf16(units)` keeps its one pass from UTF-16 to unpadded UTF-8 and returns a new `TerminalText`.
- What changes: each read allocates its two buffers; a whole-document read already did, since the pool kept only 64 KiB.
- Tests: `converted_alike` and its two callers (`a_reads_text_converted_in_one_pass_is_the_two_copies_exactly`, `unpaired_surrogates_and_line_ends_convert_as_the_two_copies`) convert without a reused buffer; the ignored `measure_conversion` measures the one pass into new buffers as "after".

### Core's small checkpoint

- Today: `SrState`'s doc (`crates/verbatim-core/src/state.rs`, lines 645 to 650) requires anything that grows to be held behind `Arc` so that a checkpoint clone is small. `tests/alloc.rs` pins it with `a_checkpoint_with_text_allocates_a_small_fixed_amount` and `a_checkpoint_allocates_a_small_fixed_amount_whatever_the_ancestor_chain` (lines 358 to 387); beside them, `the_themes_fetches_change_the_state_without_allocating` (line 345) is a step test that pins the fetches' step at exactly zero bytes, an absolute figure rather than growth with the state. Five serde helpers serialize the `Arc` fields: `serialize_shared`, `deserialize_shared`, `serialize_chunk` and `deserialize_chunk` (`state.rs`, lines 1044 to 1067) and the module `shared` (`crates/verbatim-core/src/terminal_model.rs`, lines 168 and 289 to 307).
- Replacement: the three tests and the rule go; the doc says only what the state is and that the flight recorder clones it at a checkpoint, which happens once per segment of its window. The five helpers give way to serde's `rc` feature on the workspace's `serde` dependency, which serializes an `Arc<T>` as its `T`, so the dump format does not change; `cargo hakari generate` then updates `workspace-hack`. Existing `Arc` fields stay.
- Kept: the per-step tests of `tests/alloc.rs` (a focus step, a navigation step, the text steps and a terminal line each allocate the same whatever the ancestor chain), because a step whose cost grows with the application is a real cost. The module doc loses its checkpoint sentence.

### The hook's window-class cache

- Today: `crates/verbatim-ia2/src/hook.rs` keeps each window's class in a thread-local `CLASSES` (line 350), a `WindowCache<Rc<str>>` set up for a hook set that hears windows destroyed (lines 556 to 567) and consulted by `wanted_kind` (lines 716 to 745), so a flood of events from one window reads its class once.
- Replacement: `CLASSES` and its set-up and clearing go; `wanted_kind` loses its `classes` argument and keeps its closure that reads the class at most once per event, now with `class_of(hwnd)` into an `Option<String>`, returning a `String` where it returns an `Rc<str>`. `wanted_kind`'s doc, which says it allocates nothing (line 707), is corrected. `WindowCache` stays for the arbitration (`crates/verbatim-outpost/src/arbitration.rs`), where it saves a cross-process probe.
- What changes: an event NVDA's filters ask the class of (child objects for the Excel check, a foreground change, a menu event, a tooltip's show) reads it with one `GetClassNameW`, a local call. The console host's own events (`ConsoleUpdate`, `ConsoleCaret`) are kept by `is_wanted` before it asks the class. One order changes: an event that reaches the hook after its window was destroyed reads no class, where the cache answered with the class it learned earlier, so an IME candidate window's menu event arriving after the window is gone is no longer dropped by its class; NVDA reads the class when it handles the event, so this matches it.
- Tests: `a_windows_class_is_read_once_for_its_events` and `a_destroyed_windows_class_is_forgotten` go; `the_listener_reads_a_foreground_windows_class_once` stays as the once-per-event check; the other tests built on `outpost_kinds` drop the `classes` argument.

### Kept: Core's caret history and the latency ledger's last key

- `SrState::caret_history`, eight inline slots, stays: the caret and selection rework (`docs/design/caret-selection-rework.md`) deletes it, and changing its shape now would raise the dump format's version and break every earlier dump for code about to go (Fable, 2026-11-01, pending Dickson's acceptance).
- `LatencyLedger::last_key`, a time and a flag packed in one `AtomicU64`, stays: `key_seen` runs on the keyboard hook's thread (`crates/verbatim-app/src/key_reports.rs`, line 209), and `docs/design/ownership.md`, "Who may block on whom", forbids a lock there, so the atomic keeps the hook thread lock-free rather than avoiding an allocation (Fable, 2026-11-01, pending Dickson's acceptance).

### Order of work

1. The recorder's measurement and, if its gate allows, the counting writer; the checkpoint rule and its tests with serde's `rc` feature.
2. The process, supervisor, outpost and hook crates: the relay's lending, the terminal conversion without a pool, and the hook without its class cache, with the crate guides each changes (`docs/crates/verbatim-core.md`, "The flight recorder" and "The allocation invariant"; `verbatim-process.md`; `verbatim-supervisor.md`; `verbatim-outpost.md`; `verbatim-ia2.md`).
3. Documents: the ledger's removals below, the outpost conversion's figures restated, and the instruction changes in this repository's files below.

`cargo xtask ci` runs once at the end. Wake-ups and processor time are measured before and after inside the build with `measure_keys_conhost` and `measure_keys_notepad` (debug, test audio mode), with the release measurements named under each item.

### Scenarios

Run three times each at the end of the build: `conhost_full_history_flood` (whole-document frames through the relay, the conversion without a pool, and the recorder's count of large reads) and `notepad_and_verbatim_menu` (the hook's class reads for menu and foreground events). `conhost_full_screen` and `conhost_scrollback_overflow` would also cover large reads but are recorded in `docs/flaky-tests.md`, so they are left out.

## The ledger's removals

The ledger keeps what is substantially slow or can become so: calls into other processes, wake-ups, waits, timers, locks, work that grows with the application, and work that reaches milliseconds. A line that records growth with the application is kept, or restated as its bound. Removed, by entry:

- "A focus change, UIA, steady state": the `GetAncestor` in the bullet beside the calls; its locks, timer calls, wake-ups, moved-on calls and delays stay.
- "A WinEvent focus no UIA focus answers, and the gap's read": the per-event `GetAncestor` counts and the restamp's compare and stores; the locks and the timer's set and cancel stay.
- "The listener's reads in a window the outpost judges MSAA": "its message about 10 bytes larger for its answer".
- "A focus below a hung window, UIA": "or allocation" in the steady-path bullet.
- "A focus whose walk is stuck at its bound, the outpost": the bullet on copies per focus; the lock takes stay.
- "The outpost's reducer": the copy of the facts a program reads, in the processor time bullet, which keeps its intake scans; the locks and memory bullets stay, the parked event objects among them.
- "The outpost's object store": the tag added to a short list, the one pass over a message, and the `AddRef` per position copied; its locks and the release's reading of every kept node's and anchor's tags stay.
- "A key through the keyboard hook": "Neither waits or allocates", "one boxed packet each", and the scan of 1,024 trace pairs with its 16 bytes and 16 KiB.
- "Speech only the reducer makes": the speech queue's list of marks and table insert, and the bullet on a synthesizer chunk's decode and encode in microseconds.
- "A caret report, UIA": the bullet beside the calls.
- "Ordering focuses across processes": the clock read and the message size changes in bytes; what it does not catch stays.
- "One judge: reading for every node the outpost has not released": the bullet on `Outbound::waiting_nodes` is restated as taking the outbound queue's lock and walking `reported` on every accepted MSAA state change, without "allocates".
- "The task switcher's frame": the snapshot's 24 bytes and the allocation per focus change; the process handle held open per application and the lookup under a lock stay.
- "The task switcher's items, and a grid item's row and column": "Every snapshot is 40 bytes larger."
- "A window's handle in node details": the whole entry, which records only sizes and one comparison of handles.
- "Core's meeting of chains": the message sizes in bytes and "allocates nothing"; the comparisons' bound, the product of the two lines' lengths, and the duplicate's bound stay.
- "A terminal's rotation": the bullet on freeing the steps afterwards.
- "A full-screen program's exit": the paragraph on the aside screen's hashes.
- "What Core last spoke of the focus": the whole entry, whose figures are bytes and microseconds per step.
- "A terminal's reads in Core": the kept buffers' bytes in the paragraph on the outpost's conversion, which is restated for the conversion without a pool, with its whole-document time kept.
- "A terminal review command above the last read": the hash comparisons' few microseconds.
- "A terminal's command marks": the hash comparison's few microseconds.
- "The terminal memory's cost, estimated": the paragraph of sizes per type on x64, and the paragraph on the vote's and the caret line check's microseconds; the spare buffers restated for this build, and the flight recorder's bound if its item is built.
- "The listener's owed answer per UIA focus event": the whole entry.

## The cause

Every instruction that set the project's cost rules named allocations, copies and local calls beside cross-process calls and wake-ups, with no threshold, so each session treated a single allocation as a cost to avoid in code and to record in the ledger. Traced:

- The port skill's review reference (`.claude/skills/port/references/review.md`, from b4391ae5, 2026-10-15) lists "an allocation, copy, or parse on the per-event path that could be done once" as a cost finding, and the skill's costing step (`SKILL.md`) asks for what a change "adds to the per-event work".
- `REVIEW.md` from a9b3e727 (2026-10-21) until 331ca0dc (2026-11-01) failed a change for "a call, wake-up, lock, timer or allocation added on the event or speech path that the ledger does not state", so builders recorded each allocation and byte in the ledger to pass review: the window handle's 16 bytes (10c9e83a), the trace pairs' 16 KiB (8be86807), the microsecond stamps' 31 bytes (697ab4f4), what Core last spoke (d5068b48) and the listener's owed answer, written as a review fix (fb83a51a).
- The communication design's per-event cost step (step 11, 2026-10-13 and 14) gave the class cache (3f0b8002), whose stated reason was a console flood, though `is_wanted` keeps the console host's own events before it asks the class; the recorder's serializer (610f7775); and the relay's given-up buffer (4afba986). The terminal's spare pool (4de51afa) grew from Dickson's request for one pass.
- The phase 6 design's allocation invariant (b8ea103d, 2026-10-06) asked that a step's allocation not grow with the state, which is kept; its test (41cb27f6) also bounded a checkpoint's absolute size, which became `SrState`'s rule.
- `CLAUDE.md`'s rule to judge every change on wake-ups and CPU reads as covering any processor work, however small.

No memory file is responsible: `worst-case-performance.md` concerns worst cases and remote programs' instruction counts.

### Proposed instruction changes

For Dickson, not made in this build: `CLAUDE.md`'s standing rule "Judge every change on wake-ups and CPU as well as latency, and raise neither on the event or speech path unless that is the change's purpose, because a latency figure alone hides a cost paid on every event." becomes "Judge every change on wake-ups, calls into other processes, locks, measured processor time and work that grows with the application (a document, a history, a tree) as well as latency, and raise none on the event or speech path unless that is the change's purpose, because a latency figure alone hides a cost paid on every event; an allocation, a copy or a local call is not a cost to design around or to record unless a measurement shows it matters, because code shaped to avoid one adds complexity for nothing, except where `docs/design/ownership.md` forbids it (no lock on the hook thread, no allocation on the audio thread)."

In this build's documents commit, pending Dickson's acceptance:

- `.claude/skills/port/references/review.md`: "an allocation, copy, or parse on the per-event path that could be done once;" becomes "work on the per-event path that grows with the application (a document, a history, a tree) and could be done once, or that a measurement shows costs;".
- `.claude/skills/port/SKILL.md`, the costing step: "and what it adds to the per-event work and wake-ups of the listener, the outpost, or Core" becomes "and what it adds to the wake-ups and locks of the listener, the outpost, or Core, and to per-event work that grows with the application".
- `docs/performance.md`, after its first paragraph: "The ledger records what is substantially slow or can become so: calls into other processes, wake-ups, waits, timers and locks, work that grows with the application, and work that takes milliseconds. A single allocation, a type's size or a local call of microseconds is not recorded."
- `docs/crates/verbatim-core.md`, "The allocation invariant": keeps the per-step assertions and drops the checkpoint's bound and the rule that whatever grows is held behind `Arc`.

`REVIEW.md` already says this since 331ca0dc and needs no change.

## Limitations

- If the recorder's gate keeps `HeapCounter`, the hand-written serializer remains, about 270 lines, for its constant cost per string.
- The relay's copy of a whole-document frame adds about a millisecond, off the reducer's thread, on the rare reads of a whole document.

## Decisions

Dickson, 2026-11-01:

- Simplify the flight recorder's size estimate, the relay's large-frame hand-over, the terminal conversion's spare buffers, Core's small-checkpoint rule, the hook's window-class cache, Core's caret history and the latency ledger's last key.
- Keep `ForegroundView`'s `ArcSwap`.
- Remove every inconsequential entry or figure from the ledger, which is for substantially slow things such as cross-process accessibility calls.
- Find the cause of those entries and propose the wording of each instruction responsible, with changes to `CLAUDE.md` and memory files as proposals.

Fable, 2026-11-01, pending Dickson's acceptance: the caret history and the last key are kept, for the reasons above; the recorder's estimate is built only if its measured gate allows.
