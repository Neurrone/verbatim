# Review of the hook's hold decision design, and of the "Simpler owners" edits

Reviewed on 2026-11-01 against main 39dce0b2 plus the uncommitted edits to `docs/design/logic-foundations.md`: the section "The hook's hold decision (a build of its own)" and the third build's "Simpler owners". Read only; nothing was built or run, so every counterexample below is a sequence traced through the code at the lines given.

Important findings: 5. Three are in the hold decision, two in "Simpler owners".

The hold decision itself works: the machine can decide each transition whole from the event, its source and the permit, and every rewrite `hold` makes today (`crates/verbatim-input-windows/src/hook.rs` 275 to 310) can be expressed as a machine verdict. The line citations in "The decision today" are correct, and the NVDA citations are correct line by line (below). The defects are a rule the merged swallow set needs and does not state, a cost the latency section leaves out, and a known defect in this mechanism that the design defers.

## Important

### A key whose release is swallowed after its repeats passed stays down, and the design defers it

- Where: `docs/design/logic-foundations.md`, the hold decision's "Limitations", second item, against `hook.rs` 277 to 280 and 285 to 290; the same defect for a bound gesture's key is in `crates/verbatim-input/src/state.rs` 537 and 588.
- Defect: a key whose press was held keeps its entry in `held_downs` when holding switches off, so its later auto-repeats reach the application and its release is swallowed, leaving the key down in Windows' key state. The design records this as a follow-up, but the standing rule says a defect found in the mechanism being built is fixed in that build, and this build rebuilds exactly this set.
- Counterexample, traced through the code. A search box has the focus and Down Arrow, which moves the focus into the box's result list, is held down:
  1. The first press: the machine passes it with `kb:downArrow` observed. In `hold`, `held` is true (switch on, caret table, `may_hold` true), so the key goes into `held_downs` (293) and is swallowed as `Held(1)`.
  2. Core reads the caret and the shell sends Down's press and release tagged with place 1. `decide` takes the line up to 1 (357 to 361) and passes both. The focus moves to the list, which has no text; the reducer's thread stores the switch off (`verbatim-app`, `main.rs` 1599).
  3. 500 ms later the first auto-repeat arrives. `held` is false (`KeyHolding::holds` reads the switch off), the line is empty so `deferred` is false, and `hold` returns `Hold::No`: the press passes to the list, and so does every repeat after it.
  4. The user lets go. `hold` finds the key in `held_downs` (277) and swallows the release.
  The list received Down presses with no release. Because the low-level hook swallowed the release, Windows never records it: `GetAsyncKeyState(VK_DOWN)` reports the key down until Down is next pressed and released. The same happens with permit `Defer`, where the repeats are deferred and sent again but the release is still swallowed. It also happens with the permit lost because a window has come to the front.
- The machine has the same defect for a `Decided` entry. Verbatim+Down Arrow is pressed (swallowed, in `swallowed_downs` at 537), the Verbatim modifier is let go, and Down's auto-repeats outside a text field pass (`on_press` returns `Decision::pass_observed`). The release is swallowed at 588.
- NVDA has the same behaviour, so the fix is a departure from it. A key goes into `trappedKeys` when a script runs (`nvda/source/keyboardHandler.py` 314). A later press of that key that runs no script passes, with nothing taking it out of the set. Its release is then swallowed because the key is in the set (411 to 413).
- What to do: fix it in this build, where the two sets become one. A press of a key whose entry exists, if the machine passes or defers it rather than swallowing or holding it, removes the entry, so the release takes the same path as that press. Before anything is built, ask Dickson whether he accepts this departure from NVDA. The enumerated test cannot find this defect as described. It checks the order of transitions only "while the permit holds", and it does not check that every press reaching the application is followed by its release once the user releases the key. That invariant should be added, along with a permit change while a held key is down.

### The merged swallow set does not say how `Held` and `Decided` replace each other, and one reading changes the multi-press count

- Where: `docs/design/logic-foundations.md`, "The model", the "One swallow set" item and "A key repeat"; `state.rs` 537 and 617 to 620.
- Defect: today a key is treated as auto-repeating if and only if a `Decided` swallow has happened since its last release, because `count_press` reads only `swallowed_downs`. One entry per key with one reason reproduces that only if `Decided` always wins: a bound gesture's swallow replaces `Held`, and a later hold never replaces `Decided`. The design says neither, and its "the entry staying Held" suggests the opposite.
- Counterexample one, where a bound gesture keeps `Held`, the natural reading of "the entry staying Held". Down Arrow is held in Notepad, so its entry is `Held`. Caps Lock goes down and Down's first auto-repeat completes `kb:downArrow+verbatim`. `count_press` finds no `Decided` entry and counts a fresh press, repeat 0, as it does today. 33 ms later the next auto-repeat finds the entry still `Held`, counts another fresh press, and gives repeat 1, then 2. Today `swallowed_downs` contains Down after the first, so every repeat carries 0 (617 to 620). A double-press or triple-press meaning would run from a key held down.
- Counterexample two, where a hold replaces `Decided`. Caps Lock+Down is pressed and swallowed (`Decided`, repeat 0, `last_gesture` stamped). Caps Lock is let go, and Down's next auto-repeat is held, setting `Held`. Caps Lock goes down again within 500 ms of the stamp. The next auto-repeat counts as a fresh press with repeat 1, where today Down is still in `swallowed_downs` and the count stays 0.
- What to do: state the rule, that `Decided` replaces `Held` and `Held` never replaces `Decided`. Add Verbatim+Right Arrow as a bound gesture and an auto-repeat (a second press with no release between) to the enumerated test's alphabet. Have the test compare the emitted repeat counts with today's machine-plus-hook, because the current alphabet binds no caret key.

### Passed-on keys would each read the window in front and Num Lock, a cost the latency section does not state

- Where: `docs/design/logic-foundations.md`, "The model", the paragraph after "What stays in the hook", and "Hook latency and wake-ups"; today's early return is `hook.rs` 354 to 362.
- Defect: under the gate as written, a `PassedOn` key costs one `GetForegroundWindow`, one `GetKeyState` and a clock read. The gate reads the window "when Core's view lets keys be held and either the focus has text or the machine's line is not empty", and a key comes back passed on only while the line is not empty. `KeyInput` carries Num Lock and the time for every source. Today a passed-on key returns before any read.
- Counterexample: Shift+Right is held in Notepad. The shell sends Shift's press, Right's press, Right's release and Shift's release, all tagged (`docs/design/key-holding.md`, "The shell"). Each comes back through the hook while place n is in the line, so the hook reads the window four times and Num Lock four times, for a key the machine then only passes. Every deferred transition costs the same when it is sent again. The latency section counts extra reads only for "releases, modifiers and Verbatim's gestures in a text field". The measurement plan judges the result against the runs' spread, so the cost could pass without being named.
- What to do: have the hook decode `PassedOn` first and call the machine without gathering anything (no permit, no Num Lock, no time), as it returns early today. Otherwise, count these reads in the latency section and in the ledger's "A key through the keyboard hook".

### Moving the focus group's remaining tries into the intake does not remove the hand-over in `finished`, and removing it changes what is heard

- Where: `docs/design/logic-foundations.md`, "Simpler owners", "The focus group's remaining tries kept in the intake"; `crates/verbatim-outpost-logic/src/lib.rs` 950 to 953, 1122 to 1142 and 1146 to 1150.
- Defect: lines 1122 to 1142 are not a reordering of two queues. They give the group's remaining entries to the gap read pushed during the try, so that the read, if it reports, ends the group. That decision is about the group, not about where the queue is kept. So "the reordering goes" and its "about 40 lines" are wrong, and building it as written either drops the decision or misorders the queue.
- Counterexample: a focus group has newest entry N and older entry O. While N is tried, the worker drops a copy for UIA with `CopyDropped::ReadNow`, and `gap_note` pushes `GapRead` to the front of `after` with N's group and no older entries (950). N's try finishes neither reported nor claimed, so `finished` moves `[O]` into the read's group (1136 to 1138) and does not call `continue_group`. Today the read runs. If it reports the focus, the group ends and O is never tried.
  - With the hand-over removed and the tries simply put at the front of the batch, `continue_group` queues O as well.
  - If O is placed after the read, it is tried after the read reported. If O then reports, an older focus is announced after the newer one.
  - If O is placed at the very front, it runs before the read: the older focus is tried first, and the read is made after it.
  - Either way the order and the speech differ from today.
- What to do: keep the hand-over and say so: the remaining tries go into the intake, with a gap read placed ahead of them, and `finished` still gives the read the group's remaining entries. Correct the benefit and the line count, or drop the item if moving the queue alone saves too little.

### Removing `sync_marks` and `Applied::adopted` removes the only thing that updates the mailbox's copy of the marks

- Where: `docs/design/logic-foundations.md`, "Simpler owners", "One copy of the marks"; `crates/verbatim-outpost-logic/src/lib.rs` 844 to 851 and 917 to 927; `crates/verbatim-outpost/src/outpost/shell.rs` 737.
- Defect: `sync_marks` updates the intake's copy and is also the only emitter of `Effect::Marks`, which the shell turns into `mailbox.set_marks`, the push-time copy that `drops_on_arrival` judges. The trigger at 844 depends on `applied.adopted`. The design deletes both and states no new trigger for `Effect::Marks`. It does state one for the gap timer: "given when the record's due time differs".
- Counterexample: in a console host terminal a program marks the window's console updates absorbed (`change.absorb`), and the push-time copy drops them on arrival. Later the owed read or an entry that clears the absorption sets `facts.marks.absorbed = None` (1212 to 1214 or 1403 to 1405). Today `sync_marks` emits `Effect::Marks` there. With no emitter, the mailbox keeps dropping that window's console updates, and the terminal's new output is not spoken. The same applies to a focus adopted into a console host, where the system caret's events go on being judged against the old marks.
- What to do: state that `Effect::Marks` is given whenever `Facts::marks` differs after a transition from before it, as the gap timer's effect is, and count it in the item. The benefit claimed, that "the intake can no longer plan a batch against marks the facts have moved past", does not hold either. Every write to `facts.marks` is followed by `sync_marks` in the same transition today (849, 1214, 1405), so no such state is reachable now. The benefit is one holder and fewer lines.

## Checked and found sound

- The hold decision's equivalence for the other sequences traced:
  - A held caret key, its release before or after the pass-on, and a letter deferred between them.
  - A gesture behind a held key: swallowed, never deferred.
  - The Verbatim modifier swallowed, or passed and deferred in share mode, and Caps Lock exempt as a lock key.
  - A held key's auto-repeat, each repeat held in its own turn.
  - Shift's release deferred behind a held Shift+Right.
  - Num Lock passing at once behind a held key.
  - `Own` input (only the Control tap in `verbatim-gui`, `foreground.rs` 52 and 58) neither held nor deferred.
  - A passed-on key with place 0, the modifiers sent around a held key, which changes nothing.
  - A dropped passed-on key, cleared by the next key that comes back.
  - The harness's tag (`0x5645` in the top 16 bits) cannot collide with `PASSED_ON_TAG`'s (`0x5642`).
- The hook's timeout and Core not answering: the procedure's work is unchanged apart from the reads above, and the design adds no lifecycle state and no bound of its own.
- Wake-ups: none are added. The reports and posts are the same and in the same order.
- The counts: the hook's 11 fields go to 7 with the first question accepted, and the machine's go from 8 to 9.
- NVDA citations:
  - `nvda/source/keyboardHandler.py` 93: `trappedKeys`.
  - 314: added after a script runs.
  - 319: added for the NVDA modifier.
  - 411 to 413: a release swallowed when its key is in the set.
  - 226: injected keys ignored while `ignoreInjected` is set.
  - 796 to 811: `send` inside `ignoreInjection()`.
  - `nvda/source/editableText.py` 181 to 188: the caret is read, then `gesture.send()`.
  - All match the pinned source (732d1ce2). NVDA's caret keys go into the same `trappedKeys` as its other scripts, which supports one swallow set.
- The gap timer derived from the gap record. Traced:
  - A record answered and another opened before the queued bound entry runs: the entry finds the newer record not yet due, and the newer record, with no note, is still timed by `next_timer`.
  - Restamping keeps the due time and the note.
  - The tests' seam: `TestGapTimer::set` rounds up and fires only once the time has passed, so the re-set that today follows an `Err` repeats a time already set when the newer record opened.
  The removal is safe, subject to the two notes on this item under "Nits".

## Nits

- "Simpler owners", the gap timer: say that `next_timer` skips a record whose bound entry is queued. Otherwise, while that entry waits behind a marker, `next_timer` returns a time already past and the reducer wakes again at once for a wake it will not act on, which the design principles forbid.
- The derived `Effect::GapTimer` would also emit `GapTimer(None)` when the bound's read closes the record, which today emits nothing. That is harmless, but it changes the exact effect lists some tests assert.
- The enumerated test: with about 11 to 14 steps to choose from and sequences of up to six, the test runs 1.8 to 7.5 million sequences. In a debug build, where each `on_key` builds and parses a gesture id, "seconds" is doubtful. State the alphabet, whether sequences share their prefixes, and the measured run time.
- "Hook latency": modifier presses already read the window today, through translation (`typed.rs` 150, before the modifier's own state is set). The new reads are releases, presses while Control or Alt is down, Verbatim's gestures and the passed-on keys.
- "Tests": `a_key_with_a_meaning_outside_the_text_is_not_watched` (`hook.rs` 920) tests `may_edit`, which moves into the machine, but it is not listed among the tests that move.
- "States and lines": the machine's count of 9 assumes the question on Num Lock is accepted. Without it, the count is 10.
