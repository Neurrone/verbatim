# The input build

Status: built on 2026-10-30 and amended on 2026-10-31, when the hook thread came to take only keys. Designed against main at 2cd08d08. It applies [the ownership model](ownership.md)'s rule "One route and one sequence for user input" and the hook's part of "Who may block on whom" to the keyboard hook, the gesture router, the control plane's gestures and the speech manager. It ran beside [the messages build](ownership-messages.md). The refactor preserves what is heard, except the kept change this build made: a command pressed while a caret key is held runs behind that key. That reversed one limitation the key-holding design recorded ("Verbatim's own commands ... are not deferred", `docs/design/key-holding.md`, "Limitations"). The mechanism is in the crate guides of `verbatim-input-windows`, `verbatim-app`, `verbatim-core`, `verbatim-speech` and `verbatim-model` (`docs/crates/`); this document keeps the problems, the choices and the limits.

## The problems

- Two minters of the key number. The hook minted one for each press, and the control plane's `send_gesture` minted from the same counter on its own thread and posted the gesture to the router separately. The control plane could take n and be pre-empted, the hook take n+1, cancel speech through it and post, and the control plane then post n, whose speech was fenced and silent (design review 3, finding 5).
- A global side table of trace to key number (`KEY_ORIGINS`), taken one to three times per key on the hook thread and by the reducer for every `Speak`. The reducer holding it and pre-empted at normal priority left the hook, at `HIGHEST`, waiting on a thread that was not running: a priority inversion on every key.
- A second lock on the hook thread: the hook's cancel sent on a crossbeam channel whose receiver, the speech queue thread, took the channel's waker mutex around each wait. The communication design's claim that the hook never blocks on a lock was not true.
- Two routes into Core. Key reports went from the hook straight to the reducer's port while bound gestures went through a router thread, so a review command and a caret key had no common order (ba7064e5 and its reversal e180f14a). A review command pressed while Down Arrow was held for its caret read ran before the key reached the application and read the old line.
- Any thread holding the speech manager could speak, though by decision of record only the reducer does.

## What was chosen, and why

- The hook thread is the one minter. It mints a number for each key press in the order it handles keys, and it takes nothing from another thread but the message that stops it. Reason: speech is fenced by key order, and only numbers minted by one thread in its own handling order agree with the order the inputs reach Core. `KeySeq` lives in the platform-neutral `verbatim-model`, so that only the hook mints it is a review rule; tests construct it freely.
- Gestures are real keys. The control plane has no request that injects a gesture; a test presses a gesture's keys through `SendKeys`, the Verbatim modifier as Insert, and the hook numbers and decides them as a person's. The NVDA capture tool's `--gesture` became `--open-menu`, a control-plane request that opens the menu as a tray click does, with no key number. Consequences: a scenario's gesture is a numbered input, needs the unlocked interactive desktop any key does, and the same gesture pressed twice within 500 ms is a double press, as for a person.
- One route. The hook reports everything through its one callback to the reducer's port, in the order decided: the router thread, its channel and the injection path are gone. An idle waiter goes straight to the reducer's port, naming the number of the last harness input, and the reducer holds it until the hook's `Handled(n)` mark, posted behind every report that stroke caused.
- Core owns the trace-to-key table (`SrState::key_origins`), written by `reduce` from every user input and read only by Core; `reduce` stamps every `Speak` with the number of the key that caused it as its last step, preferring the innermost `Effect::ForKey` trace, then the input's. Reason: one writer on one thread, with no lock.
- The speech queue's inbox is a completion port behind two traits (`SpeechInbox`, `SpeechInboxTaker`) that the platform-neutral speech crate defines, so the hook's cancel is a post that takes no lock and never waits, in the one first-in, first-out order the fence depends on.
- A review command or script pressed while a caret key is held waits in Core's held-key line behind that key's read, its pass and its watch, at most 100 ms after the pass, counted from its own press. Reason: NVDA runs scripts one at a time in the order queued. It departs from NVDA in what it reads: NVDA reads the old line, Verbatim the line the key moved to (a capture recorded "line one" with the keys back to back and "line two" with 300 ms between them, `docs/parity/input.md`), which Dickson's kept change names. A command waiting keeps Verbatim from being idle.
- The router's work became Core effects run by the reducer's shell: the clock read, the GUI commands, and the lock keys. A lock key's announcement is due 30 ms after the press, carries no key number so a key pressed meanwhile does not silence it (NVDA's behaviour), and is read with `GetKeyState` on the reducer thread. Harness marks and the idle round wait behind a pending announcement.
- `SpeechManager::new` returns a `SpeakToken` that only the reducer thread holds and every speaking function takes, so "Only the reducer speaks" is a type. The settings window's sound preview keeps its own function.
- The hook's code moved to `verbatim-input-windows`'s `hook.rs`, its closures to `verbatim-app`'s `key_reports.rs`, and `cargo xtask ci` gained a hook rule over those files: no lock, wait, log, I/O or sent message, an allow-list of `use` paths, test modules skipped. It reads text, so a type allowed in could still offer a locking method, which is a review rule.

Alternatives weighed:

- A barrier key for the idle waiter, a key Verbatim injects and swallows: `SendInput` fails silently when blocked by UIPI and a hook that exceeds `LowLevelHooksTimeout` is silently removed, so the key could be lost and its waiter time out.
- Numbering gestures in Core while the hook numbers keys: two minters again; avoiding that would move every cancel into Core's order, changing what is heard on every key.
- A standard channel and `MsgWaitForMultipleObjectsEx` on the hook thread: it keeps a cross-thread hand-off, a refused-after-exit case and a second wait on the hook thread.
- The hand-written lock-free inbox this build first had, to inject gestures into the hook thread: removed for its hand-written concurrency, which keys alone make unnecessary.

## Limitations

- Each post to the port allocates a box, so the allocator's own synchronisation stays on the hook thread.
- Keys the control plane injects with `SendKeys` carry no harness number, so an idle waiter is not ordered behind them; only `measure_keys_text_box` sends such keys.
- Speech of a key more than 1,024 keys old carries no number.
- A command pressed while a caret key is held waits up to the key's read bound (150 ms) and then up to 100 ms for its watch, where before it ran at once.
- The lock-key announcement waits for the reducer thread: a reducer stalled past 30 ms delays it.
- The single minter is a review rule, and the hook rule reads text.
- How M15's braille gestures, which are not keys, are numbered against the keys is left to M15's design (Dickson, 2026-10-31).

## Dickson's decisions

- The time, menu and tray-list gestures wait behind a held key as review commands do, matching NVDA's one script queue; queued behind a held key, the key says nothing and only the time is heard (2026-10-30).
- A command behind a held key, or pressed after the key was passed on while its watch is open, waits for the watch, so what it reads does not depend on timing (2026-10-30).
- The hook thread takes only keys: the injection inbox, its injector and `WM_APP` wake, `KeyReport::Barrier` and the control plane's `SendGesture` were removed, and the control protocol is version 3. A test's gesture is pressed as its keys in every mode (2026-10-31).
- Scenario gestures count repeated presses within 500 ms as a person's do; a step meant as a first press follows a different gesture or key (2026-10-31).
