# Ownership audit: Core and the other threads of verbatim.exe

Read-only, 2026-10-30, main at 5797c075. Scope: `verbatim-core`, `verbatim-model`, `verbatim-input`, `verbatim-input-windows`, and in `verbatim-app` the reducer thread, `held_keys.rs` and the router, plus how Core's state reaches the speech manager and the hook. The Core side of the caret rework, and the milestones that change this area, are covered too. The relay and supervisor, the listener and the outposts belong to the other three audits. Here they appear only where Core's state reaches them.

Terms:

- A view is a copy of an owner's state that the owner publishes to another thread, for that thread to judge something against.
- A keyed view is a view that the reader applies only while a fact it observes itself matches the key the view carries.
- An ordered view travels on the same channel as the items it is used to judge, so it can never be older or newer than they are.
- A back-channel is a view that is neither keyed nor ordered. This is the study's sense: `CoreFocus` is the example.

## Decisions and findings first

- Inside verbatim.exe the model mostly holds already. The reducer is pure: every field of `SrState` is `pub(crate)`, and there is no clock or global in its non-test code. It has one ordered intake, its completion port (15cae8a8 fixed a random select between two channels). Every view it gives another thread is either keyed or ordered. Keep these as they are.
- The one back-channel in this area is `tell_core_focus` and what feeds it: `focuses_queued`, `held_focus_owner`, `notice_recipients` and `HeldFocus::for_outpost`. Dickson's decision of 2026-10-30 (repeats read in full and judged in Core) removes it. Evidence: 43de3932, 8fe59b1b, d893e07f, c0bbf19b, the repeat hole, and review findings 1, 3 and 5 of `core-flow-review.md`.
- Core decides the foreground in five places. This is the one fact inside Core that is decided in two places or more, and it carries the longest defect chain. Change it to one record (evidence below).
- Proposed changes in this area, each with its evidence:
  - Carry a key's sequence number on the input it caused, and remove the `KEY_ORIGINS` side table.
  - Give review commands the same route into Core as key reports.
  - Make the shell's asks answer "nothing" when they are dropped.
- Everything else here is "keep as is".
- The study's rules do not cover two kinds of message my area depends on:
  - configuration views (`Fetches`, the hook's map and config, and in M5 the rule tables and the modules' settings view);
  - the order of user input across its sources (keyboard, control plane, M5 module commands, M15 braille keys).

  The section "What the rules do not cover" proposes a rule for each.

## Threads of verbatim.exe in this area

- The hook thread (`verbatim-input-hook`). It owns the decision machine (modifiers, multi-press, Verbatim keys), the typing translation (dead keys, the input method, the layout) and the physical line of held and deferred transitions (`outstanding`, `held_downs`, `sequence`). It reads three views: the gesture map, `KeyHolding` and `ForegroundView`. It sends the speech cancel and pause straight to the speech queue, gestures to the router, and key reports to the reducer's port.
- The router (`verbatim-router`). It owns the 30 ms lock-key delay and the marks queued behind it. It reads Windows state (`GetKeyState`, the clock) and turns gestures into `Input::Command`, GUI commands, or the time and lock-key inputs.
- The reducer thread (`verbatim-reducer`). It owns `SrState`, the request table (`requests.rs`), the shell's held-key line (`held_keys.rs`), the shell-items asks, the flight recorder (behind a mutex, for the panic hook), the status mirror and the readiness flags. It publishes the views.
- The speech queue, synth and audio threads. They own the lanes, the utterances handed on, the pause state and the playback position. They read `FocusNow`, which arrives ordered on their own channel, and the theme, through a `RwLock` the GUI writes.
- The GUI thread and the control plane. Changes they make reach Core as inputs. The exceptions are the theme, the synthesizer settings and the configuration file, below.
- The relay and supervisor are another audit's. My area's link to them is `send_views`: attention and held nodes, `NodesHeld`, and `Fetches`.
- Planned threads: the extension host in Core (M5: loader, directory watch, compilation), the console thread (M10), and the braille output and display-input threads (M15).

## The facts, their owners, copies and defects

### 1. Core's focus and its line

- Owner: `SrState::focus` (`FocusContext`), written only by `reduce`.
- Copies:
  - `FocusNow`, to the speech queue, as an ordered view (the `DropExpiredSpeech` effect on the queue's channel);
  - `CoreFocus`, to the listener and outposts (`tell_core_focus`), a back-channel;
  - `KeyHolding.on`, a view the hook reads (fact 4).
- Decided twice: repeats. Core's `is_duplicate_focus` decides them, and so do the readers' drops against `CoreFocus`.
- Defects:
  - 43de3932 and 8fe59b1b: a repeat was dropped where NVDA speaks it.
  - d893e07f and c0bbf19b: the back-channel was generalized.
  - The repeat hole.
  - Review findings 1, 3 and 5 of `core-flow-review.md`.
  - b94d2ea2: Core and the speech manager compared identity by different rules. That is now one rule, `ObjectKey::is_same_object`.
- Verdict:
  - `FocusNow` complies. Keep it: one writer, and it is ordered with the `Speak` items it judges.
  - The speech manager's validity test is a judgement about words made outside Core. It is allowed, because it depends on playback, which only the speech owner sees, and it takes only facts sent one way.
  - Remove `CoreFocus` and `focuses_queued`, as Dickson decided on 2026-10-30.
- Compile-time rule: keep the Core-to-process message enum to queries, `NodesHeld` and configuration views (see the rules section), with an `xtask` listing of its variants. Delete `SrState::held_focus` and `held_focus_owner`, so that no API exports Core's focus for a reader.

### 2. The foreground (inside Core)

- Owners, five fields of `SrState`, all written by `reduce`:
  - `attention`;
  - `foreground` (an `ObjectKey`, from a focus report marked `foreground`);
  - `foreground_changed_at`;
  - `foreground_window` (the listener's `ForegroundWindow` fact, used for the hook);
  - the top-level window of `latest_focus` (the stale rule).
- Two processes report the foreground by two routes: the listener's `ForegroundWindow` and the outposts' foreground focus reports. `classify` adds a second foreground test.
- Defects:
  - b1917eed, and review finding 2, which undoes it.
  - Audit difference 12.
  - 06594d66 and 9661f0d8, the same-window exception.
  - The Settings chain: 5f1c8e81, 3578a437, e60a7a58 and 38cab859. `settings_system_page` failed 10 of 10 runs before it.
  - The title lost in a shared batch (`text_box_editing`), now settled by Dickson's 2026-10-30 rule that a window's title is always said.
  - Before 8e27671d the hook hooked the foreground itself, a sixth view. That commit moved it into Core.
- Verdict: change it. Use one foreground record in Core, fed by full facts, with one test. Attention, speech validity, the stale rule and the hook's view all become functions of it.
- Compile-time rule: one `Foreground` type in `verbatim-core`, with private fields, one `apply` and read-only accessors. Then a second notion cannot be added without editing that module.

### 3. Repeats and the focus taken

Covered under facts 1 and 2. After the change, `is_duplicate_focus` is the only repeat judge, from full facts: `earlier`, plus the MSAA address and the simple-child mark. Nothing outside Core counts the focuses Core has taken.

### 4. Whether keys are held, and the hook's view of the foreground

- Owner: Core decides both, through `holds_caret_keys` (the `text_focus` test) and `hook_foreground` (the integrity and console thread, from `foreground_window`).
- Copies:
  - `KeyHolding.on`, a relaxed `AtomicBool`;
  - `ForegroundView`, an `ArcSwapOption`.

  The reducer thread writes both after every input (`main.rs` 1692 and 1693).
- How the copies are protected:
  - `ForegroundView` is keyed. The hook applies it only while the window it names is the window `GetForegroundWindow` returns.
  - `KeyHolding.on` is not keyed. A key held for a focus that turns out to have no text is passed on at once by Core. That is the stated correction, and its cost is one swallow and one `SendInput`.

  The two stores are not atomic together. I checked both orders of a torn read, and the window key makes each safe.
- Defects:
  - 8e27671d removed the hook's own foreground hook. No defect came after it.
  - c04584db: an elevated window lost its keys before the guard existed.
- Verdict: keep as is.
- Compile-time rule, only if a new view is added (M5, M8): split each view into a publisher, which is not `Clone` and is owned by the reducer thread's struct, and a reader that has no setter. Today `set` is `pub`, and any holder of the `Arc` could write. No defect shows that happening.

### 5. The held-key line

There are three records, each of a different fact:

- the hook's `outstanding` and `held_downs`: physical order, and whether a later transition must be deferred;
- the shell's `HeldKeys.line` (`held_keys.rs`): the strokes to send, in order;
- Core's `SrState::held_keys`: which key is being read, the keys waiting, `awaiting_watch`, the 100 ms bound.

How they stay consistent:

- The hook reports `Held` and `Deferred` in key order.
- Only Core decides when a held key goes (`Effect::PassHeldKey`, with the bound as `Effect::BoundHeldKeyWait`, a timer the shell keeps as an effect).
- The shell sends each key tagged with its place in line. The hook clears that place and every place ahead of it when the key comes back, so a key Windows dropped heals at the next key.

Defects:

- c04584db: keys overtook a held key.
- d7ec6b04: a wait of 10 seconds behind a key that moved nothing, and the lock keys.
- 04214e7f and 49fcca0e: keys were reported to two owners, the terminal's outpost and Core. A character deferred behind Up Arrow reached the outpost first. The cure was one owner, Core.

Verdict: keep as is. Each record has one writer, and only Core decides. The stated limitations stay: keys are lost if Verbatim exits while they are held, and Verbatim's own commands are not deferred (fact 7).

### 6. Speech cancel, pause, and which key caused an utterance

- Owners:
  - The hook decides a key's cancel and Shift's pause, by `DecisionConfig`'s interrupt settings, and sends them straight to the queue.
  - Core decides `StopSpeech`, foreground cuts and expiry.
  - The speech queue applies both, fenced by key number (bf4fbedb).
- The cause key is not carried with the speech. The hook writes `(trace, key)` into the global `KEY_ORIGINS` ring (a `Mutex`, 1024 entries), and the control plane's injected gestures write to it too. The reducer looks up every `Speak` there (`key_of`, `main.rs` 1828).
- Defects: bf4fbedb, where speech caused by an earlier key was heard after a later key's cancel.
- Verdict: two deciders, reconciled by the fence. Keep, since NVDA cancels at the key. But change how the key number travels.
- Proposed change: put the key number on `KeyReport`, `EmittedGesture` and the `Input`s they become, and let Core put it on `Effect::Speak`. Evidence, so that each part can be checked:
  - The hook takes a lock the reducer also holds for a scan of up to 1024 entries on every `Speak`. That contradicts `docs/design/communication.md` 6.2 ("never blocks on a lock"), and Windows removes a hook past `LowLevelHooksTimeout`.
  - The lookup misses for inputs that have no recorded origin. The router's lock-key announcement mints a fresh trace (`report_toggle_key`), so a later key's cancel cannot fence it. In that case the fence of bf4fbedb fails silently.

  No observed defect yet. The change is small.
- Compile-time rule: a `KeySeq` newtype that only the hook's key-order counter mints. Every user-input variant of `Input` carries one, and `Speak` takes `Option<KeySeq>` from the input that caused it.

### 7. The order of user input into Core

There are two routes from the hook to the reducer:

- Key reports go straight to the reducer's port: typed text, caret keys, held and deferred keys, clearing keys, `SpeechCancelled`.
- Gestures go through the router, which turns review and navigation keys into `Input::Command`.

So a review command and a caret key pressed one after the other have no common order in Core.

Defects:

- ba7064e5 put a press time on commands so they could tell which caret their key found. e180f14a removed it a day later as the wrong rule, because NVDA runs scripts in key order.
- The key-holding limitation: a review command pressed while a held key waits runs before the key reaches the application.
- `docs/design/communication.md` (step 8) defers the choice "when the command set is settled". M5's module commands and M8's remapping settle it.

Proposed change: one ordered route. The hook posts gestures to the reducer's port, in key order with its key reports. The router's work becomes reducer effects: the time read, the GUI commands, and the 30 ms lock-key delay as a timer effect. Then Core can queue a command behind a held key, as it queues keys (`Effect::ForKey`). Evidence: the two rounds and the limitation above. This change is optional if Dickson accepts the limitation for M5's commands too.

### 8. Requests from Core and from the shell

- Owner: `RequestTable`, "the single owner of exactly one outcome per query". It answers "gone" when an outpost ends and "unanswered" at the deadline. It complies at run time. Keep it.
- `ShellAsk` (the shell-items dialog) and the control plane's dump and idle waiters are answered by code paths, not by type.
- Defects:
  - 886d8e73: the dialog stayed enumerating forever when the taskbar's outpost ended without a replacement.
  - 75e4500f: a cancel forgot an overtaken read, and its answer, carrying the echo, was dropped.
- Proposed change: `ShellAsk` and the waiters answer "nothing" or "gone" when dropped, so a path that forgets them cannot leave a dialog waiting. Evidence: 886d8e73.

### 9. Configuration

- Owners, by section of the `ConfigStore` (an `Arc<Mutex>`):
  - the reader settings: the reducer thread alone, since the Terminal page sends a change command (`terminal_settings.rs`);
  - the synthesizer settings: the speech settings host (`persist_fn`);
  - the theme: the GUI thread (`themes.rs`). It writes the speech manager's `ThemeHandle` (an `RwLock`) directly, and sends `Input::Fetches` to Core separately;
  - the hook's `DecisionConfig` and the gesture map: read once at start, with no writer after that.
- Copies:
  - `Fetches`, to every outpost (`send_fetches`), a configuration view;
  - the theme, held twice: as presentation in the speech thread and as fetches in Core. The two are not ordered with each other or with `Speak`.
- Defects: none recorded. Keep as is for now.
- Two notes:
  - A theme switch is not an input, so the flight recorder cannot replay the words a theme chose.
  - The reducer thread writes the settings file under the store's lock on a toggle key (`main.rs` 1870). That contradicts 6.6 ("no thread on the delivered path writes a file under a lock"). It is rare and no defect is shown. Report it, and do not change it.

### 10. Readiness and the status mirror

- `listener_ready`, `focus_known`, `spares_ready` and the GUI are combined into Ready (`harness.rs`). The outposts' status is a `Mutex<HashMap>` mirror written by the reducer thread and read by the control plane.
- Defects:
  - ff25f3dd: Ready came before Core knew the focus.
  - The listener's Ready reversed twice (11f2d80a, 7e80b866, 5c64da12). That belongs to the listener's audit.
- Verdict: keep as is. These are harness facts, each flag has one writer, and Ready is computed in one place.

### 11. Core's own text, caret and terminal state

This covers `caret`, `caret_history`, `pending_caret`, `superseded_caret`, `terminals` and `say_all`. Each has one writer, the reducer. ec7c88c8 and 49fcca0e moved the terminal's rules and keys into Core, and no defect chain followed, which is evidence for one owner fed one-way facts. The caret's places of state are next.

## The Core side of the caret rework

- It already fits the model:
  - Core keeps one caret per focus, the newest by the stamp of when its read began;
  - every word is chosen in Core;
  - the outpost holds the last read and judges evidence;
  - the bounds are effects or the outpost's own.

  It removes Core's `caret_history`, the time-zero record, the superseded slot and the prediction of what Backspace removes. These are two of the nine places where the caret is kept, and the known defects come from their disagreements: "Control+A then a repeated focus says nothing", and the stale caret after a superseded answer.
- Two things must change with the 2026-10-30 decision:
  - "A repeat the focus pipeline rework sends unread is not read and leaves the last read as it was" becomes: a repeat is read in full. Its read compares with the node's last read and reports the changes. Core's choice between a repeat and a new focus decides whether those changes are spoken. This fixes the Control+A defect by construction.
  - The status line says the rework "relies on" unread repeats and the removal of `CoreFocus`. It now relies on full reads and that removal.
- Facts that must travel with the report, not be worked out again in Core:
  - The auto-repeat mark. The hook tracks it (`held_downs`), so the held-key report should carry it.
  - The edge facts. The before read carries them.
  - The editing chords' gesture, from the hook.
- The last read lives while Core holds its node (`NodesHeld`). That depends on the release rule by message sequence (another audit's fact 10). It must be built before or with the rework's step that adds the last read.
- Keys behind an edge key are not held for its answer. That is decided in Core's `HeldKeys` alone, so the hook needs no new view.

## Planned milestones

- M4.3, settings and indications.
  - New shared facts: live hook settings (the multiple-press timeout, handling keys from other applications, the Verbatim keys), the speech-expiry setting, review-follows-focus, tooltips and notifications, the audio device, trimming, and the volume of sounds.
  - Rules:
    - Each setting has one owner: the thread that applies it.
    - The hook's settings and gesture map are published as one snapshot, the map and config in one `ArcSwap`, and loaded once per key, so a key never sees half a change.
    - "Cancel speech for expired focus" is applied by Core leaving out `DropExpiredSpeech`, not by a flag the speech thread reads, so the setting has one place.
    - The status cues that "play through key presses" are decided by the speech queue from the utterance's group, carried on the utterance. That complies.
- M8, profiles and configurability.
  - New fact: the active profile. Triggered profiles depend on the focus application and say-all, which are Core's facts.
  - Rules:
    - Core decides the active profile.
    - A switch is one reducer transition. Its effects are the theme, the synthesizer settings, the reader settings, the hook snapshot, the outposts' configuration views and the modules' settings view.
    - The theme and synthesizer effects are ordered on the speech channel before any `Speak` the trigger caused.
    - Each view carries the profile's generation.

    Without this, a focus's speech can be presented under the other profile's theme. The race is concrete: the theme is switched today from a thread other than the one that queues speech.
  - Gesture remapping republishes the hook snapshot under the same rule.
  - The dialog's edits go through the configuration's single owner, as the Terminal page's do.
- M11, audio formatting.
  - New fact: a theme that maps roles to earcons and shorter speech. That is a decision about words made in the speech presenter.
  - Rules:
    - Keep presentation a pure function of the utterance and the theme, so D12 holds.
    - Make the theme switch an `Input` and an effect ordered with `Speak`, so the recorder replays it and no utterance is presented under a theme whose fetches it was not read for.
- M15, braille.
  - New threads: display I/O (possibly a host process, as for synthesizers, where a vendor driver can hang), translation, and display key input.
  - New facts: the braille buffer and its regions, tethering, routing keys, and the cell-to-text map.
  - Rules:
    - Core alone decides content and tethering (`Effect::Braille` with a generation).
    - The braille thread executes and translates.
    - A routing key or display gesture enters Core's one port as an `Input` carrying the buffer generation it was pressed on, and a key sequence number from the same counter as the keyboard, so that it fences speech as a key does. This is a keyed view in reverse.
    - Speech-braille sync takes the speech owner's mark reports as inputs.
    - No braille thread reads `SrState`.
- M5, the Core side of extensions.
  - New: the loader and directory watch in Core, the merged rule and command tables, module output effects, `on-command` round trips, `pass-through()` and sleep mode.
  - Rules:
    - The loader is the one owner of the tables. Each published table set carries a version, and an outpost or listener applies a new version between two events. The design already says this for the swap.
    - The study's "kept verdicts dropped on reload" becomes one input to the judge.
    - A module's `speak` and `cancel-speech` are facts. Core judges them against attention and `foreground_changed_at` at reduce time, as background speech is judged (15cae8a8).
    - An `on-command` is a request in `RequestTable`, with one outcome.
    - A key whose fate depends on a module's answer, `pass-through()`, goes through the held-key line, so keys behind it keep their order.
    - Sleep mode is a Core fact, published to the hook as a keyed view, keyed by the foreground window as `ForegroundView` is.
    - Commands should enter by the single route of fact 7, or the limitation is restated for module commands.
- M10, the console thread in Core.
  - Rules:
    - It never reads `SrState` shared. Its `state`, `focus` and similar variables come from a query the reducer answers with a snapshot.
    - Its node reads are requests in `RequestTable`, from the reducer thread.
    - Its changes are recorded inputs. The design already says this.

## What the rules do not cover

- Configuration views from Core to other threads and processes: `Fetches`, the hook snapshot, rule tables, and the modules' settings view. They are neither queries nor `NodesHeld`. Proposed rule: one publisher per view, a version on every publication, applied between two events, and a fact carries the version wherever its judgement depended on one.
- Views inside one process. `FocusNow`, `KeyHolding` and `ForegroundView` are copies of Core's state, as `CoreFocus` was. Proposed rule: a view is allowed only if it is ordered, or keyed with a stated correction when it is stale. Otherwise it is a back-channel. `KeyHolding` meets this by Core's at-once pass-on, and the other two meet it as stated in facts 1 and 4.
- One order for user input. Every input source takes its sequence number from one counter, and enters Core by one route, so that the speech fence and script order hold across sources: keys, control-plane gestures, module commands, braille keys, and later touch and mouse. Evidence: bf4fbedb, ba7064e5 and e180f14a, 04214e7f and 49fcca0e.
- Blocking on owner threads. The hook's "never takes a lock" is a rule, but nothing enforces it, and `KEY_ORIGINS` breaks it. An `xtask` or clippy `disallowed-types` check on `verbatim-input-windows`'s hook module could enforce it.
- Lifecycle at exit: held keys are lost when Verbatim exits. It is a stated limitation and needs no change. A typestate for the shell's line would make the flush explicit if it is ever wanted.
