# Review of the design "Failure paths and locks"

Eleven Important findings.

Reviewed: the section "Failure paths and locks (a build of its own)" of `docs/design/logic-foundations.md` (working tree, 2026-10-31), against the crate split's code in the worktree `verbatim-wt-split` (branch `split`, head dcb9be0b). Paths below are that worktree's. Design line numbers are those of `docs/design/logic-foundations.md`.

## Does each part reach its goal

- Fault points: partly. The registry and the guard are sound, but arming by count cannot pick which call fails, so three of the named tests cannot reach the path they name (finding on arming), and one failure path the logic-tests design lists, the hook's post, is dismissed on a wrong premise (finding on the hook).
- Wakes decided by reading the state: the relay mailbox, the audio notices, the cancel token and the readiness signal reach it, with one lost answer in the mailbox; the listener's watched read needs a key the design does not give and costs lock acquisitions it does not count.
- The switch's responder: reaches it for every drop path; it cannot clear the mark while a synth thread holds the request in a driver that never returns (a nit, pre-existing).
- The replacement worker that cannot start: the recommended outcome is right, but the mechanism that ends the outpost is not designed, and the obvious one never exits.
- The debug lock check: does not reach "a failure in every debug run": its panic is swallowed where nearly every application call is made, it does not see the calls the outpost's own code says reach the application, and its record is lost when guards drop out of order.

## Important findings

### The hook's post can be refused, and a refused key report is dropped silently

Design line 655. Defect: the design says the hook "sends on an unbounded channel" whose send "fails only once the router has gone"; the hook in fact posts every report through `Poster::send` to the reducer's completion port, which a refused post fails, and the report is then dropped.

Evidence: `crates/verbatim-input-windows/src/lib.rs` lines 23 to 27 ("callbacks that post to completion ports, the reducer's and the speech queue's"); `crates/verbatim-app/src/key_reports.rs` lines 66 and 92, `let _ = commands.send(...)`, where `commands` is a `Poster<ShellCommand>`; there is no router and no channel on that path.

Counterexample: the port refuses the post of a `KeyReport` carrying an end-to-end harness number (the refusal the design itself gives a fault point and an `Outbox` for on the relay's side). The report is dropped; the reducer never records the number; the control plane's `AwaitIdle`, which waits until that numbered input has been handled (`crates/verbatim-app/src/harness.rs` lines 12 to 15), is never answered, and a released modifier's report lost the same way leaves Core's view of held keys wrong. The new `Fault::PortRefusesPost` in `Poster::send` would fire on this path too, so the design adds a point that reaches the hook without a test or a rule for it. The logic-tests design's "a full channel (0d1e7802)" becomes "the hook's post refused"; it needs a rule (keep and retry as the relay's `Outbox` does, or state it as a limitation) and the test the logic-tests design asks for.

### The relay mailbox loses "the owner has gone", and `Supervisor::shutdown` then waits for ever

Design line 681. Defect: replacing the crossbeam channel with a queue under a lock removes the only signal that the owner thread has ended, and the design adds no closed state, so `Mailbox::send` succeeds into a queue no one will take.

Evidence: `crates/verbatim-supervisor/src/relay.rs` line 90 returns `OwnerGone` only because the crossbeam receiver was dropped; `crates/verbatim-supervisor/src/lib.rs` lines 714 to 723, `Supervisor::shutdown`, returns a default summary on `OwnerGone` and otherwise blocks on `reply_rx.recv()`.

Counterexample: the relay's wait fails and `Owner::run` returns (`crates/verbatim-supervisor/src/owner.rs` lines 465 to 468), or a handler panics on the owner thread; the `Inbox` is dropped. The app then calls `Supervisor::shutdown`: `Mailbox::send` locks, finds the queue empty, pushes `OwnerEvent::Shutdown(reply_tx)`, posts a wake that the port, still alive through the mailbox's `Arc<Port>`, accepts, and returns `Ok`. The `reply_tx` lives in a queue kept alive by the supervisor's own `Mailbox`, so it is never dropped and `reply_rx.recv()` never returns. The queue needs a closed mark set when the `Inbox` drops, under the same lock, with `send` failing and the queue's items dropped then; that is a lifecycle state the design does not list.

### The watched read's facts moved into the readers' state need a key per read, not per application

Design line 687. Defect: the design moves the window, top-level window, judgement and claim into "the read's entry in the readers' state" without saying what identifies a read, and the only map there today, `ReadersState::watched`, is keyed by pid, while today's per-read `Arc` is what keeps an abandoned read from touching its successor's facts.

Evidence: `crates/verbatim-listener/src/focus_read.rs` lines 315 to 317 ("Each read has its own, so an abandoned read that finds its window later changes nothing the watchdog judges"); line 1693 `watched: HashMap<Pid, WatchedRead>`; lines 1795 to 1803 remove the entry on an abandonment.

Counterexample: the read R1 of application P passes its deadline in a hung call; the watchdog abandons it, removing P's entry, and the schedule starts a thread whose read R2 of P inserts a new entry for P. R1's call returns and its walk calls `learn(hwnd1)` (line 1513): it takes the readers' lock, finds P's entry, which is R2's, records `hwnd1` as R2's window and sets R2's front judgement from it. The watchdog then judges R2's grace against a window R2 never read, abandoning a read that is on time or keeping one the user has left. The entry must be keyed by the read (its marker or thread generation), and a learn or a placement whose key has no entry must do nothing.

### The listener's change adds readers'-lock acquisitions on every judged read that the design does not count

Design lines 687 to 689 and 698. Defect: the design counts only wakes for the listener, but moving the window, top level and claim under the readers' lock makes every access the reading thread makes mid-read a lock acquisition, contended with the watchdog's tick, which holds the same lock across `moved_on`'s calls and `read_at_bound` for every read under way.

Evidence: the reading thread touches these facts at `crates/verbatim-listener/src/focus_read.rs` line 2117 (`known`, every read), line 1513 (`learn`), line 908 (`window`), line 917 (`place`), and lines 2125 to 2131 (`window`, `top_level` after the read); today none of them takes the readers' lock, and `on_learn` (lines 2207 to 2218) takes it only when the front is elsewhere. The watchdog's tick holds it at lines 2269 to 2300.

Counterexample: a focus read whose sender is not a window, on a read starting while another application's read is past its grace: the reading thread now takes the readers' lock for `known`, again for `learn` with two local calls inside, again for `window` and `place`, and once more after the read, up to five acquisitions where today there are none before the `Returned` hold; any of them waits whenever the watchdog's tick is running. `docs/performance.md` must state the acquisitions per read, and the measurement must include the reading threads' latency, not only their wake-ups; or the design keeps the reading thread's own facts (window, top level) as the reading thread's and moves only what the watchdog decides on.

### Ending the outpost when its replacement worker cannot start is not designed, and the obvious way never exits

Design lines 673 and 734. Defect: the design says the shell "ends the outpost as failed" and the outpost "reports its end with the reason", but there is no fatal end message in the protocol and no hand-off from the reducer thread to the I/O thread, and the orderly end that exists joins every worker, including the abandoned one that is, by construction, stuck.

Evidence: `crates/verbatim-model/src/protocol/mod.rs` lines 1120 to 1125, `OutpostToCore::Fault` is "worth reporting without dying"; `crates/verbatim-outpost/src/outpost/mod.rs` lines 1919 to 1987, `run_pipe` ends only on Core's `Shutdown`, a closed command pipe, or an unwatchable target, then calls `shut_down`, whose `stop` (lines 1876 to 1891) joins every worker thread; `crates/verbatim-outpost/src/outpost/shell.rs` line 757, a replacement is started only by `Effect::Abandon`, after a worker was abandoned in a call that has not returned.

Counterexample: worker generation 0 hangs in the application's provider; at its deadline the reducer abandons it and `spawn_worker(1)` fails. If the shell makes the I/O thread stop so that `run_pipe` returns, `shut_down` reaches `stop`, joins generation 0, and waits for the hung application; the process never exits, its pipe never closes, the supervisor sees only missed pongs, and Core's request table answers nothing until the hang check ends it, several ping intervals later. The design must say which thread ends the process and how (for example `end_without_detach` without joining abandoned workers, or a new fatal message that the supervisor answers with a kill), what the in-process test observes as "the outpost reports its end", and what `Shell::start` does when the first worker, generation 0, cannot start (`shell.rs` line 530 calls the same `worker::spawn`, which the design changes to return an error without naming that caller). Note for the question to Dickson: the alternative needs no new code, since `Shell::dispatch` already answers a program whose generation has no worker (`shell.rs` lines 818 to 829).

### Arming by count cannot choose which call fails, so three tests cannot reach the path they name

Design lines 665, 672, 674 and 675. Defect: `arm(fault, times)` fails the first `times` hits of a point anywhere in the process, but these tests need a particular call among several that reach the same point.

Counterexamples:

- `a_focus_handler_left_by_a_failed_setting_up_is_kept_until_the_registering_thread_ends`: the defect of 61a4bfa1 needs a registration with two focus handlers whose first add succeeds and second fails, then the first's removal fails (`crates/verbatim-uia/src/subscribe.rs` lines 1139 to 1167). Armed by count, `AddFocusHandler` fails the first add; `focus_handlers` is empty, `close` removes nothing, `RemoveFocusHandler` is never hit, its guard's exactly-once assertion fails and the kept count is zero, not one. The test cannot pass as specified.
- `a_group_whose_removals_all_failed_is_removed_when_the_registering_thread_ends`: `close` removes `registered` before `unremoved` (`subscribe.rs` lines 1287 to 1290), so the second armed `RemoveGroup` hit fails the new focus's group, while the group the move failed to remove is removed at its second try. One group still outlives the close, so a live-registration count passes, but the path named, a group whose removals all failed, is not the one taken.
- `a_launch_whose_wake_the_port_refuses_is_taken_at_the_heartbeat` and `a_shutdown_whose_wake_the_port_refuses_still_ends`: under the new mailbox only a send into an empty queue posts. If `note_views`' item is still queued when `ensure_spawned` sends (`crates/verbatim-supervisor/tests/target_gone.rs` lines 285 to 286 send them back to back), `ensure_spawned` posts nothing and the armed refusal falls on the next post, a launch helper's result from another thread. Both tests then pass through a different post on different runs, against `docs/testing.md`'s one fixed code path. The same holds for `a_message_the_port_refuses_is_delivered_again_in_order_and_shutdown_ends`, which cannot choose whether `Started` or `Ready` is refused.

The arm needs a selector the test can fix: a number of hits to let through first, or a fault point that names its call site (for example `Fault::RelayWake(Sender::Shutdown)`), and each test must state which call fails.

### Executables do not keep both feature variants, and the binaries left after `cargo xtask ci` carry the fault points

Design line 657. Defect: the claim that "both variants stay cached under `target/debug/deps` by their feature hash" holds for libraries only; on MSVC targets cargo names a path package's executables without a hash, so the two variants overwrite each other.

Evidence: the main tree's `target/debug/deps` holds `verbatim.exe`, `mockapp.exe` and `verbatim_agent.exe` with no hash beside the hashed test executables.

Counterexample: `cargo xtask ci` runs `cargo test --workspace` (`xtask/src/main.rs` line 369), which unifies `faults` into `verbatim-process`, `verbatim-uia` and the rest for every target it builds, including the binaries it builds for integration tests; `target/debug/verbatim.exe` and `target/debug/mockapp.exe` then carry the points. The suite's `cargo build -p verbatim-app ... -p mockapp` (`crates/verbatim-e2e/src/scenario.rs` lines 2779 to 2793) recompiles each binary crate whose dependencies changed, not just a relink, on every alternation; and a run that stages an existing build instead (the `VERBATIM_EXE_ENV` path the harness offers, line 2803) stages binaries with the fault points, which Dickson decided the end-to-end build never carries. The design should measure the recompilation as a cost, and either have the harness refuse to stage a build made with the feature or keep the feature out of the binaries `cargo test` builds.

### `cargo xtask ci` no longer compiles or lints the configuration that ships

Design lines 657 and 666. Defect: both of ci's builds, `cargo clippy --workspace --all-targets` and `cargo test --workspace` (`xtask/src/main.rs` lines 359 to 369), activate dev-dependencies and so always build `verbatim-process`, `verbatim-uia`, `verbatim-listener`, `verbatim-outpost` and `verbatim-supervisor` with `faults` on; nothing in ci builds them with it off.

Counterexample: a `use` needed only by a `#[cfg(feature = "faults")]` block, or an `Err` variant constructed only there, is an unused-item warning in the feature-off build, which clippy's `-D warnings` never sees; a reference to `kept_for_tests` outside its `cfg` compiles in ci and fails only in the end-to-end or release build, on GitHub's e2e job at best and not at all for the ARM64 release build. ci needs one feature-off build of the crates with points (for example `cargo clippy -p` of those crates without `--all-targets`), and its time added to the measured ci time.

### The lock check's panic is caught where nearly every application call is made

Design lines 705 and 712. Defect: the check panics at the counted call, but the worker runs each program inside `catch_unwind` and UIA's callbacks run inside `guarded`, so in both places the panic becomes a dropped entry and a log line, not a failure.

Evidence: `crates/verbatim-outpost/src/outpost/worker.rs` line 388; `crates/verbatim-uia/src/com.rs` lines 64 to 72; `crates/verbatim-uia/src/bounded.rs` line 91.

Counterexample: a worker takes `store`'s lock and, under it, makes a UIA call. The check panics; `catch_unwind` catches it; the program is reported finished with `panicked: true`; the reducer answers the request as failed and goes on. In the end-to-end suite the user hears nothing for that event and the scenario may still pass; in a mockapp test that does not assert that exact reply it passes. The design's own test, `a_call_counted_under_an_outpost_lock_panics`, runs on a test thread with no `catch_unwind` and so does not show this. The check must abort the process, or the catch sites must re-raise the check's panic in debug builds, and a test must show the check failing from inside a worker's program.

### The lock check does not see calls the outpost's own code says reach the application

Design lines 704 to 708 and 729. Defect: the check sees only calls that `verbatim_uia::calls::count` and `verbatim_ia2::calls::count` count, which by their own definition leave out making an agile reference and releasing an object, and it sees nothing of a call made on another thread for this one.

Evidence: `crates/verbatim-uia/src/calls.rs` lines 15 to 19 (taking an agile reference is not counted); `crates/verbatim-outpost/src/outpost/store/mod.rs` lines 869 to 870 ("Made before the lock: making an agile reference can call into the element's process"); `crates/verbatim-uia/src/bounded.rs` lines 64 to 107, whose operation runs and counts on the bounded thread, and adds to the waiting thread's count with `calls::add`, not `count`, and only when it answers in time.

Counterexamples: a change moves `element.agile()` inside `self.store.lock()`, the exact hazard that comment guards against; no `count` runs, no panic. A worker drops a `Vec<Agile<Kept>>` while holding a `Lock`, releasing objects of the application's process under it; no panic. A thread holding a `Lock` calls `context.bounded.run(...)` (`crates/verbatim-outpost/src/outpost/read.rs` line 1496) and blocks up to its deadline while another thread calls into the application; no panic. Either the check also fires on agile-reference creation, object release and a bounded wait, or the design states these as limitations, since the goal stated is "no call into an application is made under one of the outpost's locks".

### The lock check's record is lost when guards are dropped out of order

Design line 705. Defect: each guard restores, on drop, the location it replaced, which is correct only for last-in-first-out drops, and Rust lets guards drop in any order.

Counterexample: `let a = lock_a.lock();` records A, saving none; `let b = lock_b.lock();` records B, saving A; `drop(a);` restores none while B is still held; a UIA call now is counted with no location recorded, and the check passes a call made under B. No out-of-order drop was found in the outpost today, but nothing in the wrapper or the `clippy.toml` rule prevents one, and the check would then pass silently. The record should be a count of `Lock`s held per thread with the innermost (or every) location, so the check holds whatever the order of drops.

## Nits

- Design line 692: `listener_ready` and `focus_known` are both set on the reducer thread (`crates/verbatim-app/src/main.rs` lines 1118 and 1594), so the three flags are not each set by a different thread; the racing setters are the reducer, the GUI thread (`gui_handle`, line 447) and whoever reports the spares. The rule still holds provided `gui_handle` and the outposts' map are written before the readiness lock is taken; the design should say so, since they stay outside it.
- Design line 691: `CancelToken::on_cancel` calls `is_cancelled` while holding the waker's lock (`crates/verbatim-speech/src/driver.rs` line 351); once `is_cancelled` takes that lock, `on_cancel` must read the field directly or it deadlocks.
- Design line 696: "the mark always clears" holds for every drop path, not while the synth thread holds the `Switch` in a driver call that never returns; worth one sentence in the limitations.
- Design line 681: `Mailbox::send` is called from the app's reducer thread on the event path (`note_views`, `ensure_spawned`); the lock it now takes replaces a lock-free send and belongs in the measurement's list.
- Design line 690: every device arrival, including capture devices becoming active, now costs one audio-thread pass while a device is open; accurate as "rare", but the measurement should name it.

## Lifecycle states

Removed: the relay's `posted`; the watched read's `claim`, `front_judged` and `front_elsewhere` atomics (moved, not gone: they become fields of a per-read entry); `CancelToken::cancelled` as a separate atomic (moved into the lock); the readiness flags as atomics and `signalled` (moved into one lock); the crossbeam channel of the relay mailbox.

Added, each with whether it can go:

- The relay queue's closed mark: not in the design but required (finding on `OwnerGone`). Cannot go while `Supervisor::shutdown` must answer after the owner ends.
- The per-read entry in the readers' state and its key: required by the move; it could be avoided by leaving the reading thread's own facts (window, top level) on the reading thread and moving only the watchdog's decision.
- The outpost's end for a worker that could not start: removable by taking the alternative, which uses the existing dispatch path and adds no state, at the cost of a silent application.
- The `GUI`'s switch responder: replaces nothing; the `switching` cell stays, now with one guaranteed clearing path.
- In debug builds, the per-thread lock record in two backends: needed by the check; it should be a count with locations rather than a single saved location.
- With the feature only: the fault table, its guards, and `kept_for_tests`.
