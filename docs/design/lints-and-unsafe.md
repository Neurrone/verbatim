# Lints and unsafe

The design for milestone M4.9, Lints and unsafe, in [the roadmap](../roadmap.md). Non-test code is made unable to panic on data by a set of denied Clippy lints, a miss that could only be a logic bug is removed by construction rather than defaulted, unsafe code is confined to small wrapper crates, sizes that applications control are bounded, terminal alignment stops being quadratic, and the parsers get generated tests and fuzzing. It is the prevention behind [containment](containment.md) and [the guardian](guardian.md), which bound a panic's impact but make every panic in Core cost a restart. Decided with Dickson on 2026-11-05.

## 1. The lint set

Denied in non-test code:

- Clippy: `unwrap_used`, `expect_used`, `panic`, `todo`, `unimplemented`, `indexing_slicing`, `string_slice`, `arithmetic_side_effects`, `as_conversions`, `unwrap_in_result`, `panic_in_result_fn` and `exit`. Reason: each names an operation that panics, wraps or truncates silently, or ends the process outside the one exit path.
- `unreachable` only through `#[expect(clippy::unreachable, reason = "...")]`, so each one states why it cannot be reached.
- `exit` allows only `core_exit` and the guardian's own exits; the relay's `assert!`s in `crates/verbatim-process/src/overlapped.rs` become errors.
- rustc's `unsafe_code` (section 5).

Tests are exempt: `clippy.toml` sets `allow-unwrap-in-tests`, `allow-expect-in-tests`, `allow-panic-in-tests` and `allow-indexing-slicing-in-tests`, and `#[cfg(test)]` modules take `cfg_attr(test, allow(...))` for the rest. Test-only crates are exempt for now. Release builds keep wrapping arithmetic, since under the lint all arithmetic is explicit.

**Status.** The twelve crates that build on Linux, the nine platform-neutral crates and the three logic crates, are done (commits bc35085 to ed6a539), each with the lints as crate-level attributes in its `lib.rs` so CI enforces them per crate. This milestone does the Windows crates on a Windows machine, then moves the lints to `[workspace.lints.clippy]` and removes the attributes.

## 2. How each lint is fixed

Behaviour must not change for inputs that don't overflow or miss.

- **Arithmetic**: `checked_*` returning an error where a wrong result matters (offsets, lengths, sizes from outside); `saturating_*` for counts, durations, clocks and budgets; `wrapping_*` only where the arithmetic is meant to be modular (sequence numbers, hashes), with a comment.
- **Indexing**: iterators, `get()`, `split_first` or `chunks`. A miss that data can cause takes the function's existing error path, or is skipped with a debug log where the crate logs; a miss that only a logic bug can cause is removed by construction (section 3).
- **String slices**: `get()` with character-boundary checks, or `verbatim-text`'s boundary helpers.
- **Casts**: `From` or `TryFrom`; float to integer through one named helper per crate stating its saturating behaviour.
- **`unwrap` and `expect`**: an error path; a poisoned lock recovers with `unwrap_or_else(PoisonError::into_inner)`; a truly static table becomes a `const`, or a `OnceLock` built by a test-checked function.
- **Silent paths never hang or spin**: a record the audio thread cannot admit is dropped, ending its utterance; a converter that makes no progress ends its loop.
- **Exemptions are item-level only**, each with a reason: the binding and loader functions in `verbatim-input` and `verbatim-i18n` that parse fixed literals take `#[expect(clippy::expect_used, reason)]`, and the audio builder's `control`, `lane` and `speech_lane` take `#[expect(clippy::indexing_slicing, reason)]`.

### Code a planned rework rewrites

Code a planned rework rewrites wholesale is not fixed first. Each item takes an item-level `#[expect(..., reason = "rewritten by <rework>")]`, removed by that rework:

- whole modules: `crates/verbatim-supervisor/src/owner.rs` (M6.2) and the speech manager's `QueueThread` in `crates/verbatim-speech/src/manager.rs` (M6.3);
- items: the `ReducerThread` decision functions in `crates/verbatim-app/src/main.rs` (M6.2); the `caret_history` family in `crates/verbatim-core/src/editing.rs` and the outpost's text caret items (the caret and selection rework); the synthesizer host's `Host` and `Commands` and `verbatim-synth-hosted`'s `HostedSynth` state (M6's second step).

Everything else is fixed.

## 3. No fallback for a logic bug: removal by construction

Code reaches no fallback that could only be a logic bug: a miss is removed by construction, or passed up as an error, never defaulted silently. Reason: a silent default hides the bug in release and in tests alike, and a debug assertion beside a fallback still leaves two behaviours to reason about. The first lint pass kept 54 such fallbacks beside `debug_assert!(false, ...)`, read through the `proven` modules of `verbatim-core` and `verbatim-text`; this milestone removes every one, and the `proven` modules with them.

The root pattern is producers that return offsets (`line_start`, `line_end`, `crlf_lines`, `boundary`, `floor_boundary`, `words`, `grapheme_at`, `word_start`, `Line.rows`) that callers apply again. The producers return `&str` or `&[T]` instead. By technique:

- 27 sites: the finder returns the subslice or the reference;
- 13: iterators, `zip`, `split_inclusive`, or moving ownership;
- 3: merging parallel collections into one (the mixer's `SourceRun`, `Screen::trim`);
- 3: a type that owns the invariant (the alignment's `Likeness` masks, `LineState`);
- 1: a handle passed by value (`in_flight`'s abandon);
- 4: a total clamp, where clamping an outpost's data is the intent, which is not a fallback.

A rewrite with `skip` or `take` counts as removal only where the index provably comes from the same data; where it comes from another function, the producer returns the slice. The largest changes: `Outcome` and `Line` owning their rows and `NewRows`; `words` and the grapheme API returning `&str`, a change to `verbatim-text`'s public API; `Screen` and `LineState`, which are serialized; the mixer's `SourceRun`. `BufferedPiece::text` stores its own copy of its text.

Three sites cannot be removed in safe Rust, and each gets a mechanism instead:

- **`Text`** (`crates/verbatim-text/src/terminal_rows.rs`), an `Arc<str>` with a range into it, which is self-referential: it keeps one checked slice in `as_str`, and `prefix` enforces character boundaries, so `Text`'s invariant is enforced where it is made.
- **`Intake::push`** (`crates/verbatim-outpost-logic/src/intake.rs`), whose stored index survives two moving mutations: it gets stable handles, a slot collection plus an order list, in the focus pipeline rework's third build, which rewrites that code. Re-finding by identity is not enough, since "not found" would fall silently into `push_back`.
- **The alignment's `put`** (`crates/verbatim-text/src/terminal_rows.rs`), whose callers write indexes from `align::pair` into two tables: the alignment returns pair decisions that `put` walks, so no index crosses a function.

## 4. Latent bugs to fix

- `remove_client_focus` in `crates/verbatim-outpost-logic/src/intake.rs` removes an entry without adjusting `Intake::push`'s stored `place`, so the index can point at the wrong slot; safe today only because a `Foreground` fact never resolves a marker.
- The alignment's `Likeness` slot numbers saturate at `u16::MAX`, so characters beyond 65,535 distinct ones share a slot (`crates/verbatim-text/src/terminal_rows/align.rs`).
- `Text::prefix` does not check character boundaries, so `Text`'s own invariant is not enforced.

Each is pinned by a failing test first.

## 5. Unsafe code

- **`unsafe_code` is denied workspace-wide, with an allow only in the wrapper crates**, each of which wraps one Windows or C interface behind a safe API; every other crate forbids it. Reason: unsafe code reviewed in a few small places, behind types that make misuse unrepresentable, instead of spread through every Windows crate. Today `forbid` holds in `verbatim-uia-rops`, `verbatim-term-sim`, `verbatim-outpost-logic` and `verbatim-listener-logic`; after small moves, in the supervisor, the outpost (all but its `main.rs`), the OneCore driver and the synthesizer host's library.
- **Safe wrappers**, shared where today each crate repeats the calls: an `OwnedHandle`, window enumeration, `SendMessageTimeout`, a COM apartment guard and a message-only window, which remove about 60 sites.
- **Safety that today holds by convention becomes a type or an `unsafe fn`**: `RawPipe` in `verbatim-control` is `Sync` with `read(&self)`, so two readers could leave a stack `OVERLAPPED` pending; the listener's grace timer handle is `Send` and `Sync` on the promise that the grace state sets no timer after `Close`, which it breaks for `Ended`; `end_of` is a safe function that dereferences a raw pointer; `leave_mta` and `release_mta_usage` are safe functions; the WASAPI sink's `Send` is wrong on `RPC_E_CHANGED_MODE`; the OneCore driver blocks in a single-threaded apartment with no message pump; and `crates/verbatim-process/src/port_queue.rs` posts raw `Box` pointers through a completion port, which the design principles forbid.

## 6. Bounds on sizes applications control

Moved in from M6.6 (the security model's U2 to U4), because a size an application controls is a panic, a stall or an allocation Core pays for:

- **Walks** have a hop cap, a time budget and, where identity exists, repeat detection: a walk stops at an element it already visited, keyed by runtime id for UIA, and by window handle, object id and child id for an MSAA object with an address of its own. The remote operation's search for a window above the walk gets a cap of its own.
- **Lists are capped before they are fetched**, since the provider decides how much it returns and the cost is paid on fetching.
- **Strings are capped where they are read, and a cut is known**: each property at the outpost, and grapheme clusters in length during segmentation. How a cut is spoken is M6.3's.

Test: a hostile `mockapp` mode (a self-parent, a parent cycle, a very deep chain, a million children, a 30 MB name), each asserting the exact speech and that another application keeps speaking within a stated time.

## 7. Terminal alignment in less than quadratic time

Terminal alignment (`crates/verbatim-text/src/terminal_rows/align.rs`) is quadratic in both rows and row width, and Core is shared by every application, so one terminal can stall all speech (the security model's U5). It becomes:

- **identical rows matched by hash first**, which settles the common case of unchanged and scrolled rows in linear time;
- **a cap** on the rows and row width compared exactly among those left;
- **a scroll-shift fallback** past the cap, aligning the rows by how far the screen scrolled rather than row against row.

What is heard for ordinary output does not change; the existing alignment tests and terminal scenarios pin it. Test: a terminal printing very wide rows of distinct characters keeps Core's time and memory within stated bounds, measured before and after.

## 8. Generated tests and fuzzing at the parsers

Every parser of data from outside Verbatim gets generated tests (any input: no panic, no read past the end, and its stated round trip or invariant), and fuzzing, highest exposure first: the frame and raw-section decoders, terminal alignment and the control-plane reader, then the configuration and package readers and the synthesizer escapes (the security model's X12). A short run per change and a longer one nightly; each failure is kept as a fixed regression test. Kani proofs are not used; generated tests take their place (Dickson). Settings loading and applying are M8.3's.

## Limitations

- Test code and test-only crates may still panic.
- A wrapper crate's `unsafe` is only as sound as its review.
- Past the alignment's cap, rows are aligned by the scroll shift alone, so an edit that also moved rows can be read as more change than it was.

## Costs

The lints and the construction changes add no call, lock or wake-up; a checked operation costs a branch. The alignment's hashing is one pass per row, measured with the terminal flood scenarios before and after, and recorded in `books/dev/src/reference/performance.md`, since the alignment is on the event path.
