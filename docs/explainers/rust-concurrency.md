# Rust concurrency as Verbatim uses it

The Windows IPC primitives have their own explainer ([IPC](ipc.md));
this file covers the *Rust-side* concurrency vocabulary Verbatim is
written in, and how the two layers meet. It is for a reader fluent in
systems programming who has not written concurrent Rust. Unlike its
siblings, this file is specific to Verbatim: each primitive is
introduced with where the codebase actually uses it.

## The overall shape: threads and channels, no async

Verbatim uses no async runtime (no tokio, no `async fn`). Every
concurrent activity is a dedicated OS thread
(`std::thread::spawn`) owning blocking calls, and threads talk
through channels. This is a deliberate fit for the domain: the
blocking calls are Windows calls (COM, pipe reads, `WaitForSingleObject`,
synth `speak`), which an async runtime cannot await anyway without
parking a thread per call — so the thread-per-role design pays the
same thread cost with far less machinery. Examples of the pattern:
the speech queue thread and synth thread (`verbatim-speech`), the
input hook thread (`verbatim-input-windows`), one reader thread per outpost
pipe plus the heartbeat thread (`verbatim-outpost`'s supervisor), the
query-pool workers, and the agent's two tunnel copy threads
(`verbatim-agent`).

Rust's contribution is that this style is *safe*: the compiler's
`Send`/`Sync` rules make it a type error to share non-thread-safe
data across threads, so the review question "can these two threads
race on this?" is mostly answered by "it compiles."

## Channels

Two channel families appear:

- `std::sync::mpsc` — the standard library's multi-producer,
  single-consumer queue. Fine for simple pipelines; used where
  nothing fancier is needed.
- `crossbeam_channel` — the de-facto standard upgrade
  (multi-consumer, `select!` over several channels, better
  performance). Verbatim uses it wherever a channel is load-bearing:
  the outpost runtime, listener, and query pool
  (`crates/verbatim-outpost/src/query_pool.rs` uses `bounded` and
  `unbounded` plus `recv_timeout` for deadline waits).

Vocabulary that matters when reading call sites:

- `bounded(n)` vs `unbounded()`: bounded channels apply backpressure
  (a full channel blocks senders — or fails `try_send`); unbounded
  never block senders but can grow without limit.
- `send` blocks (bounded, full); `try_send` never blocks and returns
  an error when full or disconnected. The input hook uses `try_send`
  and *drops the gesture on a full channel*
  (`crates/verbatim-input-windows/src/lib.rs`) — dropping is the correct
  behavior there because the hook thread must never block
  ([Windows and messages](windows-and-messages.md) explains the OS
  timeout). When you see `let _ = tx.try_send(...)`, the dropped-work
  case is a design decision worth interrogating — the review packet's
  "silent failure" theme.
- `recv_timeout(d)` is the blocking-with-deadline receive; a
  disconnected channel (all senders dropped) ends `recv` loops — the
  usual clean-shutdown signal, no explicit stop flag needed.

## Shared state

- `Arc<T>` — atomically reference-counted shared ownership; the
  standard way to hand one object to several threads. `Arc<Mutex<T>>`
  adds mutation with a lock. Verbatim keeps `Mutex` use rare and
  small (e.g. the E2E suite's one-live-instance lock); most sharing
  is message passing instead.
- Atomics (`AtomicU64`, `AtomicBool` with `Ordering`) — lock-free
  counters and flags, e.g. the query pool's parked-worker count and
  the speech pipeline's cancellation flag that the synth sink checks
  cooperatively. For counters and flags, `Ordering::Relaxed` vs
  `SeqCst` subtleties rarely matter; treat any *pair* of atomics that
  must be observed consistently as a red flag in review.
- `OnceLock` — thread-safe lazy one-time initialization (a modern
  replacement for static init patterns).
- `arc_swap::ArcSwap` — the specialist: readers load an `Arc`
  snapshot with no lock at all, and a writer replaces the whole value
  in one atomic store. Verbatim uses it for the gesture map
  (`SharedGestureMap`): the hook thread reads the current bindings on
  every keystroke without ever contending a lock, and rebinding is
  one store. The idiom to recognize: *snapshot semantics* — a reader
  keeps a consistent (possibly slightly stale) view for the duration
  of its use, and nobody blocks anybody.

## Panics, threads, and cleanup

- A panic on a background thread kills only that thread; whoever
  joins it or holds its channel sees a disconnect. Guard threads
  whose death must be noticed (the supervisor's reader threads
  respawn outposts on end-of-stream for exactly this reason).
- `std::panic::catch_unwind` runs a closure and turns a panic into a
  `Result` — the E2E registry uses it so a panicking scenario body
  still runs teardown before the panic is re-raised.
- RAII guard structs (`Drop` impls) are the cleanup idiom
  everywhere: `Scenario`'s Drop kills every launched process,
  `InputHook`'s Drop uninstalls the hook, handle wrappers close
  their `HANDLE`s. In Rust code review, "what happens on early
  return or panic?" is answered by asking "what does Drop do?" —
  there is no separate cleanup path to audit.

## Where Rust meets the Windows primitives

Verbatim calls Windows through the `windows`/`windows-core` crates —
generated bindings where each API is `unsafe` to call and returns
`Result`-wrapped `HRESULT`s. The patterns to know when reading those
modules:

- **Handle ownership**: raw `HANDLE`s are wrapped promptly in types
  whose Drop calls `CloseHandle`; a bare handle passed across
  functions is a review smell. Job objects
  (`CreateJobObjectW` + kill-on-close in the supervisor) are pure
  RAII: dropping the job handle is what kills the child tree.
- **Blocking thread per pipe** is the default I/O style (simplest
  correct thing): a reader thread sits in `ReadFile`. Unblocking it
  is done by closing the handle or `CancelIoEx` from another thread
  (the agent tunnel), or by the peer closing.
- **OVERLAPPED I/O where duplex demands it**: a synchronous pipe
  handle serializes reads and writes at the driver level, so the
  control-plane server and the agent tunnel open pipes with
  `FILE_FLAG_OVERLAPPED` and use event-based `GetOverlappedResult`
  waits — not for async-style scalability, but because full-duplex
  (one thread reading while another writes the same pipe) requires
  it. This is documented at the two implementations
  (`crates/verbatim-control/src/server.rs`,
  `crates/verbatim-agent/src/tunnel.rs`).
- **Waitable everything**: `WaitForSingleObject` on process handles
  (did the app die?), events, and mutexes
  (`single_instance` uses a named mutex plus an event to implement
  replace-the-running-instance). The composite idiom "wait on
  {work done, cancel}" appears wherever a blocking Windows call
  needs a deadline — the same shape NVDA's watchdog uses
  ([IPC](ipc.md) covers the objects themselves).
- **COM apartments are per-thread state** ([COM](com.md)):
  `CoInitializeEx` choices are made where threads are born — the UIA
  client threads and query-pool workers each own their apartment and
  their own `Uia` client, which is why those objects are never sent
  across threads. The type system helps (COM wrappers are mostly
  `!Send`), but apartment discipline is one of the few concurrency
  properties Rust cannot fully check — flagged accordingly in the
  review packet's FFI item.

## Reading order into the code

Good first files, in order of increasing intricacy:
`crates/verbatim-input-windows/src/lib.rs` (thread + try_send + arc-swap),
`crates/verbatim-speech` (two named threads, command channels, a
cancellation flag), `crates/verbatim-outpost/src/query_pool.rs`
(bounded deadlines, worker abandonment, watchdog thread),
`crates/verbatim-agent/src/tunnel.rs` (overlapped duplex relay).
