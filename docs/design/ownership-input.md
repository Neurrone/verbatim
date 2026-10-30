# The input build

Status: design, written on 2026-10-30 against main at 2cd08d08. It applies [the ownership model](ownership.md)'s rule "One route and one sequence for user input" and the hook's part of "Who may block on whom" to the keyboard hook, the gesture router, the control plane's injected gestures and the speech manager; its brief, starting draft and known traps are "The input build" in [the ownership builds](ownership-builds.md). It runs beside the messages build. The refactor preserves what is heard, except the kept change this build makes: a command pressed while a caret key is held runs behind that key. That reverses one limitation the key-holding design recorded ("Verbatim's own commands ... are not deferred", `docs/design/key-holding.md`, "Limitations"), which Dickson's kept change replaces; nothing else recently built is undone.

## Feasibility

The goal is achievable against the current code. Every source of the key number and every route into Core can be brought onto the hook thread and the reducer's port, and nothing on the hook thread needs a lock once the trace-to-key table moves into Core and the speech queue's inbox becomes a completion port ("The speech queue's inbox", below), since today's crossbeam send to it takes a mutex on every cancelling key:

- The hook thread already runs a message loop (`crates/verbatim-input-windows/src/lib.rs`, `pump_messages`, lines 663 to 675), so other threads can post to it, and Windows serialises its low-level hook calls with the messages it takes, so numbers minted there are minted in one order.
- The reducer's port (`crates/verbatim-process/src/port_queue.rs`) is first in, first out across posters and its `send` never waits (lines 150 to 174); the hook already posts its key reports there through the closure in `crates/verbatim-app/src/main.rs` (lines 405 to 500).
- Everything the router does (`router_loop`, `main.rs` lines 2238 to 2334) is a table lookup, a GUI post that never waits (`crates/verbatim-gui/src/lib.rs`, `GuiHandle::send`, line 118), a clock read, or a 30 ms timer, all of which the reducer thread can do as effects.
- The only reader of `KEY_ORIGINS` is the reducer thread's `execute` (`main.rs` lines 1814 to 1821); every trace it looks up belongs to an input that Core reduced first, so Core can own the same table.

No sequence was found that defeats the single minter or the single route; the sequences that would break the design, and how it prevents each, are listed under "Sequences that would break it".

## Terms

- The key number: `KeySeq`, a new type in `verbatim-model` (`crates/verbatim-model/src/key_seq.rs` (planned)), a `u64` ordered by press. It replaces the bare `u64` of `next_key` (`lib.rs` lines 236 to 251) and of `SpeechControl::cancel_through` (`crates/verbatim-speech/src/manager.rs`, line 187).
- The minter: the one place that gives out key numbers, the hook thread's state (`HookState`).
- A user input: a key press the hook decides, or an input injected for the user, today the control plane's gestures and later M5's module commands, M12's remote input and M15's braille keys.
- The injector: the handle by which another thread posts an injected input to the hook thread (`Injector`, below).

## The current code

- Two minters. The hook mints with `next_key()` for each press (`lib.rs` lines 884 to 888); the control plane's `send_gesture` mints with the same atomic on its own thread (`main.rs` lines 2572 to 2575) and posts the gesture to the router separately. That is design review 3, finding 5: the control plane takes n and is pre-empted, the hook takes n+1, cancels speech through it and posts, the control plane then posts n, and n's speech is fenced and silent (`manager.rs` lines 898 to 901).
- A side table. Every key, gesture, typed text and lock key is recorded by trace in `KEY_ORIGINS`, a global mutex over 1,024 entries (`lib.rs` lines 239 to 273), taken one to three times per key on the hook thread (lines 830, 838, 895, 902, 944); the reducer thread takes it for every `Speak` and scans it (`main.rs` lines 1818 and 1819). The reducer holding it and pre-empted at normal priority leaves the hook, at `HIGHEST`, waiting on a thread that is not running: a priority inversion on every key.
- A second lock on the hook thread. The hook's speech callback calls `SpeechControl::cancel_through` or `toggle_pause` (`manager.rs` lines 187 to 193), a send on a crossbeam unbounded channel whose receiver, the speech queue thread at normal priority, waits in `queue_rx.recv()` (line 896) registered with the channel's waker. The send then runs `SyncWaker::notify`, which takes the waker's mutex (crossbeam-channel 0.5.17, `waker.rs` lines 221 to 224), and the queue thread takes the same mutex around each wait: the same inversion as `KEY_ORIGINS`, on every key that cancels or pauses speech. `docs/design/communication.md` section 6.2 says the hook "never blocks on a lock", which is not so today.
- Two routes. Key reports go from the hook's closure straight to the port; bound gestures, harness marks (`Routed::Handled`) and control-plane idle waiters (`Routed::Barrier`, posted by `IdleLine::wait`, `crates/verbatim-app/src/harness.rs` lines 167 to 193) go down an unbounded channel to the router thread, which turns them into inputs and posts them (`main.rs` lines 2273 to 2333). A review command and a caret key therefore have no common order. Inside Core a review command never waits behind a held key: only caret keys, typed text and clearing keys queue while `held_keys.busy()` (`crates/verbatim-core/src/reduce.rs` lines 120 to 131).
- The router's lock-key timer. A lock key's gesture is held 30 ms (`TOGGLE_KEY_DELAY`, `main.rs` line 2389), then the router reads `GetKeyState` and posts `Input::LockKeyToggled` with a fresh trace (lines 2393 to 2415), so the announcement carries no key and is never fenced. Harness marks and idle waiters that arrive meanwhile are held behind it (`behind_toggles` and `pass_in_line`, lines 2253 to 2292 and 2339 to 2350).
- Speaking. `SpeechManager::speak`, `speak_for_key`, `play_earcon`, `play_earcon_heard` and `play_earcon_to_end` (`manager.rs` lines 385, 408, 433, 471 and 458) take `&self`, so any thread holding the manager can speak; today only the reducer thread does (`main.rs` lines 1820, 1866 and 1868), by the decision of record "Only the reducer speaks" (`docs/architecture.md`, section 2).

## The model this build applies

### One minter, on the hook thread

The hook thread mints every key number, for its own key presses and for every injected input, in the order it handles them. Reason: speech is fenced by key order, and only numbers minted by one thread in its own handling order agree with the order the inputs reach Core.

- `HookState` keeps the last number given out, `last_key: KeySeq`, in place of the global `KEY_SEQUENCE`; `next_key` and `KEY_SEQUENCE` are deleted. A press takes `KeySeq::after(last_key)`; a release takes `last_key`, as today (`lib.rs` lines 884 to 888).
- `KeySeq` lives in a platform-neutral crate, so its construction cannot be private to the hook; that only the hook mints it is a review rule, as the model says. Tests construct it freely.
- Old flight dumps have no key fields: each such field deserialises to `KeySeq::NONE_YET` (zero) through a serde default function in the model.

### The injector

`InputHook::injector()` returns an `Injector`, cloneable and `Send`, holding the hook thread's id and a handle to the thread, duplicated by `InputHook::start` and closed when the last `Injector` goes; an open handle keeps the thread object, so its id is not given to another thread while any `Injector` can post to it. `Injector::inject(Injected)` boxes the item and posts it with `PostThreadMessageW` as a private `WM_APP` message whose `LPARAM` is the box; if the post fails, the box is taken back and the item returned as the error, so an injection reported failed never runs later. `Injected` has two variants today: `Gesture(GestureId)` and `Barrier(u64)`, the idle waiter's token.

On the hook thread, `pump_messages` (moved into `hook.rs`, below) takes each such message, reclaims the box and calls `inject(state, injected)`, which for a gesture mints the next number, carries out `KeySpeechEffect::Cancel` with it through the same speech callback a key uses (so speech is cancelled and `Input::SpeechCancelled` posted, as `send_gesture` does today, `main.rs` lines 2575 to 2578), then reports the gesture with its number; for a barrier it reports the barrier. When the loop ends, the remaining injected messages are taken with `PeekMessageW` and their boxes dropped. Apart from that drain, after the hook is removed, the hook thread runs no loop but its `GetMessageW` loop: no modal loop and no `PeekMessageW` loop, so every key and every injected input passes through the one loop, in the order it takes them.

The control plane's `send_gesture` keeps its checks (a parsable, bound, non-lock gesture, lines 2556 to 2567) and then only injects; it no longer mints, cancels or posts. `IdleLine::wait` injects its barrier instead of sending `Routed::Barrier`, so a waiter stays behind every gesture injected before it. Both get their `Injector` from `ControlHandlersConfig`, in place of `gesture_tx` and `speech_control`: `InputHook::start` (`main.rs` line 392) moves before `ControlServer::start` (line 361), so the injector exists when the handlers are built. The comment that puts the hook last, "so nothing is swallowed before there is somewhere to route it" (line 381), no longer applies: the hook routes only to the reducer's port and the speech queue's, both made before it.

### One route, to the reducer's port

The router thread, `Routed`, the hook's crossbeam channel and `gesture_tx` are deleted. The hook reports everything through its one report callback, in the order it decides: `KeyReport` gains `key: KeySeq` on `Observed`, `Held`, `Passed` and `Typed`, and three variants, `Gesture { gesture, key, pressed_at_us }`, `Handled(u64)` and `Barrier(u64)`. The lock key's gesture (`lib.rs` lines 938 to 952) becomes `Gesture` like any other. The app's closure turns each report into its `ShellCommand` and posts it to the port, as it does for key reports today. Reason: a command and a caret key then reach Core in the order pressed, with nothing between the hook and the port to reorder them.

The closure maps a gesture on the hook thread, by lookups in immutable tables captured when the hook starts, as it already looks up `caret_keys` (`main.rs` lines 388 to 421):

- the Verbatim menu gesture: `Input::Script { trace_id, key, repeat, script: Script::ShowMenu }`;
- a review or object-navigation binding: `Input::Command { trace_id, key, command, repeat }`;
- the time binding: `Input::Script { .., script: Script::ReportTime }`;
- the tray list binding: `Input::Script { .., script: Script::ShellList }`;
- a lock key's gesture: `Input::ToggleKeyPressed { trace_id, key, toggle, pressed_at_ms }`;
- anything else, nothing, as the router's `continue` does today.

`Handled(n)` becomes `ShellCommand::InputHandled(n)` and `Barrier(token)` `ShellCommand::AwaitIdle(token)`. The router's per-gesture `tracing::info!` moves to the reducer thread, which logs each `Command` and `Script` input it applies, since the hook thread writes no log.

The bounded-channel check in `InputHook::start` (`lib.rs` lines 493 to 498) and its test go with the channel: the hook's only outbound path is the port, which has no bound, and the closure that posts to it is checked (below).

### Key numbers in Core, not in a side table

`KEY_ORIGINS`, `record_key_origin` and `key_of` are deleted. Core keeps the same table, `SrState::key_origins` (`crates/verbatim-core/src/keys.rs` (planned)), the newest 1,024 pairs of trace and key number, written by `reduce` from every user input before it is dispatched, read only by Core. Reason: the table has one writer, Core, on one thread, with no lock; the number arrives on the input, and Core is where every later trace of the key's work (its requests' answers, `Effect::ForKey`) is known.

`Effect::Speak` becomes `Speak(Utterance, Option<KeySeq>)`. Its construction sites pass `None`, and `reduce` stamps every `Speak`, inside `Effect::ForKey` included, as one last pass after `terminal_model::intercept` (`reduce.rs` lines 49 to 75), with exactly the lookup `execute` makes today: the trace of the innermost `ForKey` around it, else the input's trace, else the utterance's own trace. The input's trace is the one `apply` takes today (`main.rs` lines 1662 to 1678), which moves into the model as `Input::trace_id()` so the shell's ledger and Core's stamp read one list. `SpeechManager::speak_for_key` and `SpeechControl::cancel_through` take `KeySeq`, and the shell passes the stamped number on unchanged. The compiler then requires a number on every user input and a place for it on every `Speak`; that Core stamps it is held by a unit test.

### The speech queue's inbox

The speech queue thread takes its events from a completion-port queue (`crates/verbatim-process/src/port_queue.rs`) in place of the crossbeam channel made at `manager.rs` line 294, and `SpeechControl` and the manager post to it. A post is one `PostQueuedCompletionStatus`, which takes no lock of user mode and never waits (lines 150 to 174), and the port is first in, first out across posters, so a cancel and every `Speak` keep the one order the fence depends on. Reason: the hook's cancel then takes no lock.

`verbatim-speech` is a platform-neutral crate and may not depend on `verbatim-process`, which uses the Windows bindings, so it defines the inbox as two traits over an opaque `QueueItem` wrapping its private `QueueEvent`: `SpeechInbox` (`Send` and `Sync`, `post(QueueItem) -> Result<(), QueueItem>`, never waiting) and `SpeechInboxTaker` (`take(&mut self) -> Option<QueueItem>`, waiting). `SpeechManagerConfig` takes the pair; the app builds it from `port_queue::<QueueItem>()`, and the speech crate's own tests and the device-free test configuration use a crossbeam pair, which is correct there because no hook thread posts to it. Every other sender of `queue_tx` in `manager.rs` (the manager, `SpeechControl` and the playback listener, line 789) holds an `Arc<dyn SpeechInbox>`. The queue thread's wake-ups are unchanged: one per event, now from the port.

### Commands behind held keys

`Input::Command` and `Input::Script` queue behind a held key, as caret keys do, while `held_keys.busy()` or the watch of the held key last passed on is open (`awaiting_watch`), as a new `WaitingKey::Command(Box<Input>)` (`crates/verbatim-core/src/state.rs`, lines 377 to 385). In `next_held` (`crates/verbatim-core/src/editing.rs`, lines 265 to 325) it waits as a held key waits: until the key ahead of it has had its caret read and been passed on, and until that key's watch is answered or 100 ms (`KEY_AHEAD_BOUND_MS`) after the pass, whichever is first; its effects are wrapped in `Effect::ForKey` with its trace. Its bound runs from its own press time (`pressed_at_ms`, from the hook's report), so one pressed after the pass waits only the rest of the 100 ms, and one pressed past them runs at once. `held_wait_ended` (lines 376 to 389) accepts the command's trace as it accepts a held key's. `Input::ToggleKeyPressed` never queues: NVDA starts the announcement's timer when the key is pressed, outside its script queue (`nvda/source/inputCore.py` line 640, `gesture.reportExtra()`; `nvda/source/keyboardHandler.py` lines 740 to 742).

Reason: NVDA runs scripts one at a time in the order queued (`nvda/source/scriptHandler.py` lines 242 to 244; `docs/nvda/input.md`, "Scripts run one at a time"), and its caret script says nothing once another script is queued (`nvda/source/editableText.py` lines 94 to 95 and 165 to 167). The watch's answer for the key ahead is marked as having a later key (`later_key`, `editing.rs` lines 275 to 277), so the key itself says nothing, as in NVDA. Verbatim's own wait for the watch departs from NVDA, which reads the old line (the second open question); it delivers the line the key moved to, which the kept change names (`docs/design/ownership.md`, "The refactor preserves behaviour").

A command waiting in Core's held-key line keeps Verbatim from being idle: the idle conditions gain "Core's held-key line is empty" (`HeldKeys::busy` false). Without it, a command waiting on the watch of a key already passed on would leave every condition clear, since the shell's held keys are then empty and a caret watch is a passive request (`main.rs` lines 1898 to 1899). The condition adds nothing for keys alone: while a held key is read, or waits in Core's line, it is also in the shell's held keys.

### The router's work as effects

- The time: `Script::ReportTime` emits `Effect::ReadClock { repeat }`; the shell reads `datetime::local_time` and `local_date` and applies `Input::ClockRead` with the trace it executed the effect under, after the current input's effects, so one input is reduced at a time. Its speech is stamped with the gesture's number through that trace.
- The GUI: `Script::ShowMenu` emits `Effect::Show(GuiRequest::Menu)` and `Script::ShellList` emits `Effect::Show(GuiRequest::ShellList(kind))`, the kind chosen from `repeat` by `shell_list_kind`, which moves from `main.rs` (lines 2448 to 2454) into Core with its two tests. The shell posts the GUI command, dropping it with a warning before the GUI is up, as `send_gui_command` does (lines 2379 to 2385); `ReducerContext` gains the GUI handle.
- The lock keys: `Input::ToggleKeyPressed` appends the key to `SrState::toggle_keys`, the announcements to come in press order, and emits `Effect::ReadToggleKey { toggle, due_ms }` with `due_ms` 30 ms after `pressed_at_ms` on the shared clock. The shell keeps these in a queue among its deadlines (`next_deadline`, `main.rs` lines 2070 to 2078), as it keeps `terminal_timer`; at each one it reads `GetKeyState(vk) & 1` and applies `Input::LockKeyToggled` with a fresh trace, as `report_toggle_key` does. Core removes the front announcement and speaks "scroll lock on" or "scroll lock off" as today (`crates/verbatim-core/src/shell.rs` line 27), with no key number, so a key pressed within its 30 ms does not silence it, as NVDA (`keyboardHandler.py` lines 740 to 751). A `LockKeyToggled` with nothing pending, from an old dump, is spoken as today.
- The marks behind a lock key: while `SrState::toggle_keys` is not empty, the reducer thread holds each `InputHandled` mark it takes, in order, and hands them on (setting `handled_input` and broadcasting) after the input that empties it, as `behind_toggles` does. The idle conditions (`idle_conditions_clear`, `main.rs` lines 1621 to 1627) gain "no lock-key announcement is pending", which keeps an idle waiter that came behind a lock key from being answered before its announcement, as the router's holding of `Barrier` does.

`GetKeyState` moves from the router's thread to the reducer's. Both are threads with no window that never read keyboard messages; the router's read passes `lock_key_announcements` today, and the hook thread, another such thread, relies on the same toggle bit for Num Lock before every key (`lib.rs` line 871). The scenario's three runs confirm the reducer's read. `GetAsyncKeyState` would not do: its low bit says whether the key was pressed since the last call, not whether it is on.

The variant is named for the toggle, not the lock, because the hook rule forbids the word `lock` in `key_reports.rs`, which builds it.

### The speaking token

`SpeechManager::new` returns the manager with a `SpeakToken`, the only one made; its field is private to `verbatim-speech`, it is not `Clone`, and it holds a `PhantomData<Cell<()>>`, so it is `Send` but not `Sync` and only the thread that owns it can lend it. `speak`, `speak_for_key`, `play_earcon`, `play_earcon_heard` and `play_earcon_to_end` take `&SpeakToken`. The app moves it into `reducer_loop`, which returns it when the loop ends; the exit sound is played by the reducer thread under it while the main thread waits (`main.rs` lines 1525 to 1529 and 1865 to 1866), so no speech at exit needs another path. The settings window's sound preview keeps `play_sound` with no token (`crates/verbatim-app/src/themes.rs` lines 76 to 80; 87b4f284), and the theme page's speech and event previews already go through the reducer (lines 84 to 96). Reason: "Only the reducer speaks" becomes a type the compiler checks.

### The ownership check on the hook

The hook-thread code moves out of `lib.rs` into `crates/verbatim-input-windows/src/hook.rs` (planned): `HookState`, `keyboard_hook`, `decide`, `hold`, `report_passed`, `inject` and `pump_messages`. `lib.rs` keeps what other threads call: `InputHook::start` and its drop, the thread's start-up (`hook_thread`, `Apartment`), `KeyHolding`, `ForegroundView` and `Injector`. The app's two hook closures move into `crates/verbatim-app/src/key_reports.rs` (planned). `typed.rs` returns the profile manager's failure from `Typing::with_text_services` for `hook_thread` to log, so it writes no log itself.

`xtask/src/ownership.rs` gains a hook rule for `hook.rs`, `typed.rs` and `key_reports.rs`. `Checked` gets its own forbidden list. Each file may `use` only `std::cell`, `std::collections`, `std::sync::Arc`, `std::sync::atomic`, `std::time::Instant`, `windows::Win32`, `verbatim_input`, `verbatim_model`, `verbatim_process::clock`, `verbatim_process::port_queue::Poster`, `verbatim_speech::SpeechControl` (whose cancel posts to the speech queue's port), `crate` and `super`, and may not name `Mutex`, `RwLock`, `Condvar`, `OnceLock`, `LazyLock`, `lock`, `mpsc`, `sleep`, `std::thread`, `std::fs`, `std::io`, `std::net`, `std::process`, `tracing`, `SendMessageW` or `SendMessageTimeoutW`. The check skips a `#[cfg(test)]` module, from the attribute to the brace that closes the `mod` after it, counted on code stripped of comments and strings, since the hook's tests collect reports under a `Mutex`. Like the pure rule, it reads text: a type allowed in (the latency ledger, whose `key_seen` is one atomic store) could still offer a locking method, which stays a review rule.

## Where each part runs

- The hook thread (`verbatim-input-hook`, `HIGHEST`): decides keys, mints every number, cancels speech, holds and defers keys, takes injected inputs, and posts every report to the port. Its handlers make no call that dispatches sent messages.
- A control-plane connection's thread: validates a gesture and injects it, or injects an idle waiter's barrier and waits for its answer.
- The reducer thread (`verbatim-reducer`): Core reduces, stamps key numbers and queues commands behind held keys; the shell executes the effects, keeps the lock-key deadlines, reads the clock and `GetKeyState`, posts GUI commands, holds harness marks behind a pending lock key, and speaks with the token.
- The speech queue thread: waits on its own completion port and compares each `Speak`'s number with the newest cancel's, as today.
- The GUI thread: shows the menu and the shell list when told.

## Edge cases and what is heard

- Down Arrow held in a text box on "line one" while its caret is read, and the review current-line command pressed during the read: Down Arrow is passed on, its watch answered with the caret on "line two" and marked as having a later key, so it says nothing; then the command reads the line at the caret: "line two". NVDA says "line one" here (the second open question); a capture confirms NVDA's words before any expectation is fixed.
- The same, with the application answering the watch after 100 ms: the command runs at the bound and reads the caret where the outpost finds it then.
- A review command pressed while two held keys wait: it runs after the second, in the order pressed.
- The time gesture pressed while a caret key is held: the key's watch is marked as having a later key (`later_key`), so the key says nothing and only the time is heard, such as "10:42", as NVDA.
- Verbatim+V pressed while a caret key is held: the menu opens after the key is passed on and answered, or at the bound.
- Scroll Lock, then "a" within 30 ms with typed characters spoken: "a", then "scroll lock on"; the letter's cancel came before the announcement was queued and does not fence it.
- Num Lock then Scroll Lock within 30 ms: "num lock on", "scroll lock on", in press order.
- A harness stroke of Scroll Lock followed by `AwaitIdle`: the waiter is answered only after "scroll lock on" has been queued.
- A gesture injected while say-all speaks: say-all stops and the gesture's speech follows, as a key's does.
- A gesture injected while the user's key is being decided: whichever the hook thread handles first gets the lower number; the later one's cancel fences the earlier one's speech, as a later key's does.
- A gesture injected after the hook has stopped, at exit: refused with "the keyboard hook is gone"; today the router still took it.
- Verbatim exiting with a lock-key announcement pending: not announced, as today.

## Sequences that would break it

- Two minters (review 3, finding 5): the control plane takes n and is pre-empted, the hook takes n+1 and cancels through it, n's speech is fenced. Prevented: the control plane holds no number; it posts the gesture, and the hook thread numbers it when it handles it.
- The side table's inversion: the reducer holds `KEY_ORIGINS` for a lookup and is pre-empted; the hook at `HIGHEST` waits on it for every key. Prevented: the table is Core's, on the reducer thread, and the hook rule forbids any lock in the hook's modules.
- The waker's inversion: the speech queue thread holds the crossbeam waker's mutex while it registers to wait and is pre-empted; the hook's cancel waits on it. Prevented: the cancel is a completion-port post, and the queue thread waits on the port.
- A command overtaking a caret key in transit: Verbatim+Numpad8 pressed just before Down Arrow goes by the router while Down Arrow's report goes straight to the port, so the command runs after the key it preceded. Prevented: both are posted by the hook thread to the port in the order decided.
- A command overtaking a held key in Core: Down Arrow is held for its caret read, the current-line command runs at once and reads the old line (ba7064e5 and its reversal e180f14a). Prevented: the command waits in `held_keys.waiting` behind the key's read, pass and watch.
- A command's speech losing its number: the queued command runs while Core handles the held key's watch answer, whose trace is the key's, so a lookup by the input's trace would stamp the key's number. Prevented: the stamp reads the innermost `ForKey` trace first, the command's.
- An idle waiter overtaking an injected gesture: the harness injects a gesture and then calls `AwaitIdle`; a barrier posted straight to the port could pass the gesture still on the hook thread's queue, and idle would be answered before the gesture ran. Prevented: the barrier is injected through the hook thread too.
- A mark or waiter overtaking a lock key's announcement: Scroll Lock's stroke is marked handled at once, and the harness reads speech before "scroll lock on" is queued. Prevented: the marks wait while `SrState::toggle_keys` is not empty, and so does the idle round.
- The announcement fenced: "a" pressed within 30 ms cancels through its number, and an announcement carrying the lock key's number would be dropped. Prevented: `LockKeyToggled` is not a user input and its `Speak` carries no number, as NVDA's `ui.message` after `callLater` is heard.
- An injection reported failed but run later: an item queued in a channel whose wake message could not be posted would be taken at the next wake. Prevented: the item travels in the message itself and comes back when the post fails.
- Re-entry on the hook thread: a handler that pumps sent messages would deliver the next key into `decide` while the injected input is half handled. Prevented: `inject` makes the same calls a key does, the rule forbids `SendMessageW`, and a re-entered `decide` still passes the key untouched (`lib.rs` lines 722 to 733).
- Speech from another thread: a GUI or control-plane thread calls the speech manager directly, unordered with Core's speech. Prevented: every speaking function takes the token, which only the reducer thread holds.
- A replay that stamps differently from the live run: the table is part of `SrState` and serialised with it, so a flight recorder checkpoint carries it; inputs from old dumps carry `NONE_YET`.

## The facts and their owners

- The key number: the hook thread.
- The pairs of trace and key number: Core.
- The newest cancel's number: the speech queue thread.
- The lock-key announcements to come: Core; their deadlines and the toggle state's read: the reducer thread's shell.
- The harness marks held behind them: the reducer thread's shell.
- Which commands wait behind held keys: Core.

## Limitations

- Each post to the port allocates a box, as the hook's key reports do today, so the allocator's own synchronisation stays on the hook thread; the hook takes no lock of Verbatim's.
- A thread's posted-message queue holds 10,000 messages; an injection beyond that is refused with an error. Only the harness injects today.
- Speech of a key more than 1,024 keys old carries no number, as today.
- A command pressed while a caret key is held waits up to the held key's read bound (150 ms, `main.rs` line 990) and then up to 100 ms for its watch, where today it runs at once.
- `KeySeq`'s single minter is a review rule, and the hook rule reads text.
- The lock-key announcement now waits for the reducer thread: a reducer stalled past 30 ms delays it until it recovers, where the router read it on time.

## Cost

- The router thread goes: one thread fewer, and one normal-priority wake fewer per bound gesture, the wake whose ready-to-run tail the thread model measured at 2 to 30 ms (`docs/design/communication.md`, section 6.1). A review command goes from the hook to the reducer in one hop, and its `SpeechCancelled` and the command itself are posted back to back by the hook, so say-all stops and the command runs in consecutive inputs with nothing between them.
- A lock key: the router's wake for its gesture and for its timer, then the reducer's for the announcement, three wakes, become the reducer's wake for the press and its deadline wake, which applies the announcement in the same step: two.
- An injected gesture: the hook thread's wake replaces the router's; the count is unchanged, at `HIGHEST` instead of normal.
- Per key on the hook thread: one to three uncontended lock and unlock pairs fewer, and for a key that cancels or pauses speech, the waker's mutex replaced by one port post. Per `Speak` on the reducer thread: the same scan of up to 1,024 pairs, without the lock. Per user input: one push into Core's table.
- Memory: the table, 16 bytes a pair, 16 KiB at most, as today, now inside `SrState`, so each flight recorder checkpoint (one per 512 inputs) clones it.
- Latency on the typing and caret-key paths is unchanged: their reports already went straight to the port. The builder records the reducer's and the hook's wakes and processor time per key from `measure_keys_notepad` before and after (`docs/tooling/measuring.md`), with the router's gone, in `docs/performance.md`.

## Tests

### Added

- The hook (`hook.rs` tests): a key press, an injected gesture and a second press are numbered 1, 2 and 3 in the order handled; each cancel names its own number; the reports leave in that order, the gesture's carrying 2.
- The hook: an injected gesture fences speech as a key does: it calls the speech effect's `Cancel` with its number before reporting, and a key decided after it cancels through a higher one. `speech_an_earlier_key_caused_is_dropped_after_a_later_keys_cancel` (`crates/verbatim-speech/tests/pipeline.rs`, line 881) takes `KeySeq` and holds the fence itself.
- The hook: an injected barrier is reported behind the reports of every key decided before it; an injection posted to thread id 0, which no thread has, fails and comes back as the error.
- The app (`key_reports.rs` tests, pure): each report maps to its one `ShellCommand`: an observed caret key to `CaretKey` with its key number, a clearing key to `ClearingKey` then `CaretKey`, the menu, time and tray-list gestures to their `Script`, a review binding to `Command`, a lock key's gesture to `ToggleKeyPressed`, an unbound gesture to nothing, `Handled` and `Barrier` to their marks.
- The speech crate: a cancel posted through the inbox fences an earlier key's `Speak` posted after it, through the crossbeam pair, as `speech_an_earlier_key_caused_is_dropped_after_a_later_keys_cancel` does today.
- Core (`crates/verbatim-core/tests/text.rs`): a review command pressed while a caret key is held runs after the key's caret read, pass and watch: `HeldKey` (Down Arrow, key 1), then `Command` (current line, key 2) emits nothing; the read's answer emits the watch and `PassHeldKey`; the watch's answer, caret on "line two", says nothing, then the command's read follows under `ForKey`; its answer speaks "line two" with key 2. A second case ends the wait with `HeldKeyWaitEnded` for the command's trace.
- Core (`crates/verbatim-core/tests/shell.rs`): a lock key's announcement is still heard after a key pressed within its 30 ms: `ToggleKeyPressed` (Scroll Lock, key 1, at 1,000 ms) emits `ReadToggleKey` due at 1,030; `CharacterTyped` "a" (key 2) speaks "a" with key 2; `LockKeyToggled` on speaks "scroll lock on" with no number. `toggle_keys` is pending between the press and the toggle, which the idle conditions read; a reducer-thread test of the marks' holding (a pure helper beside `idle.rs`) shows a mark taken while pending handed on after the toggle.
- Core: the stamp, with a `TextCompleted` under a key's trace stamped with that key, a `ForKey` trace preferred to the input's, an unkeyed input's speech unstamped, and the oldest pair gone after 1,024 newer; `Script::ReportTime` emitting `ReadClock`, the menu and shell list emitting `Show`, each queued behind a held key; `shell_list_kind`'s two tests, moved.
- The ownership check: a `Mutex` in a hook file is found; a `#[cfg(test)]` module is skipped to its closing brace; a `use std::sync::Mutex` is found through the allow-list.

### Removed, each with its cheaper replacement

- `what_a_key_caused_is_found_by_its_trace_until_newer_keys_push_it_out` (`lib.rs` line 970): by Core's stamp test.
- `a_bounded_channel_is_refused` (`lib.rs` line 1048): the channel is removed.
- `a_single_press_lists_the_system_tray` and `a_double_press_lists_the_taskbar` (`main.rs` lines 2652 and 2660): moved into Core unchanged.
- No app test exercises the router's relaying; its ordering cases are the Core and hook unit tests above.

Rewritten: `a_burst_larger_than_the_old_bound_is_delivered_in_order_with_none_lost` asserts the same order through the report callback; every Core and mockapp test that matches `Effect::Speak(utterance)` matches `Effect::Speak(utterance, _)`, and every one that builds a user input gives it a `KeySeq`.

### Scenarios

Three runs each: `lock_key_announcements`, `notepad_caret_keys`, `text_box_caret_keys`, `text_box_selection_keys`, `notepad_review_cursor`, `text_box_review_cursor`, `windows_terminal_typing`, `conhost_typing`, `measure_keys_notepad`; also `object_navigation_in_settings`, whose commands are injected, and `menu_and_settings_dialog`, whose menu now opens by a Core effect.

## The known traps, against the code

- Review 3, finding 5: the hook thread is the one minter, and the control plane only injects.
- Review 1, finding 8: the announcement carries no number, as NVDA's.
- Review 1, finding 9 and nits: `KeySeq`'s minting is a review rule; `GetKeyState` on the reducer thread, as on the router's today, confirmed by the scenario, not `GetAsyncKeyState`; the marks and idle waiters wait on Core's pending announcements; the `KEY_ORIGINS` sequence is the priority inversion.
- Review 2, nits: the token covers `speak`, `speak_for_key`, `play_earcon`, `play_earcon_heard` and `play_earcon_to_end`, and speech at exit is the reducer thread's; the bounded-channel check goes with the channel, and the hook rule skips test modules.

The builder updates the crate guides of `verbatim-input-windows`, `verbatim-app`, `verbatim-core`, `verbatim-speech` and `verbatim-model`, `docs/design/key-holding.md`'s limitation on commands, and `lock_key_announcements`' module comment, which names the router; the documents commit corrects the communication design's thread list, including section 6.2's claim that the hook never blocks on a lock, true only once this build's speech inbox is in.

## Decisions

- The time, menu and tray-list gestures wait behind a held key as review commands do, matching NVDA's one script queue (`inputCore.py` lines 647 to 649, `scriptHandler.py` lines 242 to 250); queued behind a held key, the key says nothing and only the time is heard (Dickson, 2026-10-30).
- A command behind a held key waits for that key's watch, at most 100 ms after the pass, so it reads the moved line as the kept change says (`docs/design/ownership.md`, "The refactor preserves behaviour"); so does a command pressed after the key was passed on while its watch is still open, which follows from the same decision, so that what it reads does not depend on timing. This departs from NVDA, which reads the old line in every case (`editableText.py` lines 94 to 95 and 165 to 175, `queueHandler.py` lines 61 to 69, `globalCommands.py` line 1976), as a capture confirmed ("line one", `docs/parity/input.md`) (Dickson, 2026-10-30).
