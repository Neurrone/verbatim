# Keys, announcements and input: merged and verified record

Area: `crates/verbatim-core/src/keys/`, `announcements/`, `changes.rs`, `caret_tell.rs`, `report_focus.rs`, `navigator.rs` (with `caret_location.rs`, `details.rs`, `review.rs`, which both reviewers counted); `crates/verbatim-input`; `crates/verbatim-input-windows`; `crates/verbatim-app/src/key_reports.rs`. Checked at commit ba89dca7. Every quotation below was compared with the file today; line numbers are today's.

## 1. Size

Recounted with `wc -l`, comments included; a file's inline `#[cfg(test)]` module counts as tests (key_reports.rs's tests start at 438, not at the `#[cfg(test)]` fault point at 77).

| Part | Verbatim product | Verbatim tests | NVDA counterpart (pinned) | NVDA lines | Ratio |
|---|---|---|---|---|---|
| Hook, decision machine, gesture tables, typed text, the hook's reports | 3,910 (verbatim-input 2,125; verbatim-input-windows 1,348; key_reports.rs 437) | 3,885 (inline 3,169; hold_sequences.rs 716) | keyboardHandler.py 960; winInputHook.py 161; scriptHandler.py 400; inputCore.py's gesture execution (about 400 of its 1,078) | about 1,920 | 2.0x |
| Core's keys: key numbers, fences, stroke line, press entry, caret keys, terminal key taking | 1,939 (entry 326, mod 121, strokes 238, terminal 93, caret_key 1,161) | 2,773 (caret_key_tests 1,893, caret_confirm_tests 645, inline 235) | editableText.py 70-382, the caret scripts | 313 | 6.2x |
| Announcements, changes, caret tells | 2,373 (announcements 1,249, changes 766, caret_tell 358) | 3,752 (announcements/tests 1,099, changes_tests 1,124, caret_tell_tests 936, replay_announcements 584, inline 9) | NVDAObjects/__init__.py 1314-1422; UIA/__init__.py 2508-2558, 2731-2800, 2811-2833; behaviors.py 346-360 and 1000-1017; editableText.py 417-480; window/edit.py 1068-1081; IAccessible `event_alert` | 380 to 530 (A counted 377; B 530, adding IAccessibleHandler's routing and the desktop-name change) | 4.5x to 6.2x |
| Report focus, caret location, navigator | 691 (210, 46, 435) | 1,641 | globalCommands.py 1515-1912 and 2956-2979 | about 420 | 1.6x |
| details.rs, review.rs | 191 | 227 | chosen inline in speech.py | about 30 | 6x |
| **Area** | **9,104** | **12,278** | | **about 2,900** (2,660 without inputCore) | **3.1x product** (2.8x to 3.4x); tests 1.35x product, against NVDA's 0.18x |

The reviewers agreed on product (both about 9,200) and tests (12,250 and 12,450); the settled figures are 9,104 and 12,278. They differed on NVDA by more than a tenth (2,470 and 3,100). B counted inputCore's gesture execution and winInputHook.py, both real counterparts of verbatim-input, so the settled figure counts them.

What the extra lines buy (both reviewers, checked):
- Hand-offs across a thread and a process: the hook's own line of places beside Core's stroke line; twelve `Request` implementations (359 lines, M2), because Core does not hold the application's objects; the MSAA confirmation stage.
- Behaviour ruled beyond NVDA: no key overtakes a held key (key-holding.md, 2026-10-22); the 150 ms release (2026-11-13); lock keys outside the queue; the console's 5 ms poll; the elevated-window guard.
- Nothing: the duplication and dead code in section 2.

Deletion target:

| | Product lines | Test lines |
|---|---|---|
| Remove now (M1, M2, M6 to M10's copies, M12 to M15) | about 970 | about 425 |
| If Dickson rules for the removal (Q3, Q4, Q6, and M3's addition) | about 225 net | about 510 |
| Planned phase (M5, gesture tables and vocabulary: about 250 and 120; M6.4, stage tables and terminal copies: about 140 and 40) | about 390 | about 160 |

After "remove now" the area is about 8,130 product lines, 2.8x NVDA.

## 2. Findings

| id | title | class | found by | file and line | what goes | product saved | test saved | speech changes |
|---|---|---|---|---|---|---|---|---|
| M1 | Presses converted back into the old `Input` variants and run through reduce.rs | Remove now | both | keys/entry.rs 187-210, 266-325; reduce.rs 104-108, 156-200, 248-296; keys/mod.rs 20-121; model event.rs 596-766 | the bridge, the 8 old variants, `KeyOrigins` once two follow-ups carry their key | about 370 | about 64 | no |
| M2 | Twelve `Request` implementations of one shape | Remove now | both | caret_key.rs 128-205; caret_tell.rs 118-148; changes.rs 237-310; selection.rs 58-121; report_focus.rs 55-86; navigator.rs 68-166 | 11 of 12 impls, into `TextRequest` and one properties request | about 200 | 0 | no |
| M3 | Report focus on Core's own focus says nothing, against the 2026-11-15 ruling | Question for Dickson (Q1) | A only | report_focus.rs 115, 126 | nothing; building the ruling adds about 10 | -10 | -20 | needs a capture |
| M4 | Gesture meaning in two tables with a silent fallback; command vocabulary written three times | Planned phase (M5) | A (tables), B (vocabulary) | key_reports.rs 34-65, 286-299, 384-436; main.rs 2123-2150; scripts.rs 53-195; keys/entry.rs 215-264 | one binding per gesture naming the model's command | about 250 | about 120 | no |
| M5 | Edit and terminal caret paths repeat the stage table and the judging loop | Planned phase (M6.4) | both | caret_key.rs 268-320, 322-375, 605-640, 716-744 | one stage enum, one loop | about 90 | about 40 | no |
| M6 | Dead model events, a copied toast class and a hashed toast identity | Remove now | A (model), both (toast) | model event.rs 367-402, 463-470, 484-497, 504-519; toast.rs 21, 69, 90-102 | 5 variants, `AlertOrigin`, the copy, the hash | about 90 | about 5 | no |
| M7 | The queue slot's fence and arrival fact are placeholders on every entry kind | Remove now | B only | entry.rs 128; announcements/mod.rs 139, 187; changes.rs 90-97, 741-742; caret_tell.rs 86; entries.rs `set_fence` | facts set once at the join | about 70 | 0 | no |
| M8 | Duplicate types in the input crates and a press kind the shell never sends | Remove now | B (A for `gesture_map_for`) | toggle.rs 105; verbatim-input-windows lib.rs 143-155; app strokes.rs 33-40; model request.rs 1521; scripts.rs 553 | `of_gesture`, `HeldStroke`, `PressAction::Passed`, `gesture_map_for` | about 70 | about 35 | no |
| M9 | Modifier state kept three times on the hook thread | Remove now | both | state.rs 542, 637, 1010-1018; hook.rs 58, 317-330; typed.rs 296-306 | the hook's Alt tracker, two copies of the modifier table | about 40 | about 40 | no |
| M10 | Acceptance re-judged at eleven call sites; the exempt list and `passes` written twice | Remove now (the copies); Q5 (judging once) | B only | announcements/mod.rs 219, 255-269, 312-326; changes.rs 324-335; entries.rs 436-457 | one exempt list, one `passes` | about 30 | 0 | no (the copies); needs a capture (judging once) |
| M11 | A missing property answer taken as "no states"; reply validation unused | Defect | both | changes.rs 207-210, 460-461 | replies decoded through `answers_in_order` once | about 5 | -20 (one test added) | only under a reader fault |
| M12 | The navigator's window is an `Option` patched after the fact; a stale comment | Remove now | both | state.rs 152-156; navigator.rs 10-13; changes.rs 517 | `Navigator::on(object, window)` | about 25 | 0 | no |
| M13 | The hook's `Decision` is eight fields whose invariants are comments | Remove now | A only | verbatim-input state.rs 234-273 | the invalid combinations | about 0 | 0 | no |
| M14 | Smaller copies: details filters, a one-variant enum, kinds re-matched, generation lookups | Remove now | both (details), B (the rest) | details.rs 26-59, 69-73; toast.rs 28 and five more; nine `OutpostId(instance.generation)` | helpers and the re-matches | about 60 | 0 | no |
| M15 | A parity entry contradicts the code and cites a test that does not exist; stale crate guide and comments | Remove now (documents) | B only | docs/parity/input.md 30-50; books/dev/src/reference/crates/verbatim-input.md 84-96, 142-144; verbatim-input-windows lib.rs 235, typed.rs 1 | the stale text | 0 | 0 | no |
| M16 | Auto-repeat not counted as a multi-press, called deliberate with no ruling | Question for Dickson (Q2) | B only | verbatim-input state.rs 104-108, 517-520; docs/parity/input.md 27-29 | nothing | 0 | 0 | needs a capture |
| M17 | An MSAA alert path no known application reaches | Question for Dickson (Q3) | A only | announcements/alert.rs 1-140 | the path, if ruled | 140 | 141 | only for an application raising an MSAA alert; none known |
| M18 | The hook keeps and re-posts refused posts | Question for Dickson (Q4) | both | key_reports.rs 13, 77-170 | `KEPT`, `Ports::post`, `try_post`, `PostSubject` | about 95 | about 120 | no |
| M19 | The terminal keeps its own copy of the deferred keys; a console caret tell is dropped twice | Planned phase (M6.4) | both (deferred), B (caret tell) | keys/terminal.rs 49-87; caret_tell.rs 207-218 | the copies | about 50 | 0 | no |

### M1. Presses converted back into the old `Input` variants and run through reduce.rs (both)

Traced from the shell. Every key the hook reports reaches Core as `Input::Queue { input: QueueInput::Press(..) }` (key_reports.rs `press`, 240-255), except a lock key, which is `Input::ToggleKeyPressed` (key_reports.rs 401-407). The shell posts these `PressAction`s: `Command` and `Script` (`gesture_command`, 384-436); `CaretKey` (from `Held`, `Observed`, `Passed` and a clearing key's second post, all through `caret_key(..)`); `Typed`; `Control`; `Clearing`; and `EditingChord`. It never posts `PressAction::Passed`: a passed key is posted as `caret_key(OTHER_KEY, None)` (key_reports.rs 343-351).

No production code constructs `Input::Command`, `Script`, `CaretKey`, `HeldKey`, `CharacterTyped`, `ControlTyped`, `ClearingKey` or `EditingChord` except `today()`. A grep of `Input::<variant>\b` over crates/, xtask/ and uia-facts/ finds them, outside the model and `today()`, only in two kinds of place:
- matches: reduce.rs, keys/mod.rs, and main.rs 1439-1443;
- tests: terminal/tests.rs 207 and 1093; tests/text.rs 3360 and 3420; tests/terminal_reading.rs, 13 sites; tests/terminal_support/mod.rs; and recorder.rs 369, verbatim-e2e artifacts.rs 728 and verbatim-replay lib.rs 380 and 397, all inside test modules.

At a press's turn, `keys::entry::run` (146-213) handles it in this order:
- Every `CaretKey` goes to `caret_key::run` first (147-150), because `CaretPress::of` (caret_key.rs 1137-1151) returns `Some` for every `PressAction::CaretKey`.
- A `Command` whose `Own::of` is `Some` runs its own function. Every bound command but the four toggles has an `Own`: entry.rs 238-262 with `review_text::handles` (review_text.rs 46-71), checked against the 39 commands of `ScriptAction::review_command` (scripts.rs 147-195).
- Everything else goes through `today()`:

```rust
    let Some(input) = today(trace_id, &press) else {
```
(entry.rs 187), then
```rust
        facts.key_origins.record(trace_id, press.key_seq);
        effects.extend(crate::reduce::waited_input(&mut facts, &input));
```
(entry.rs 207-208).

What is reachable in production, from the shell's inputs, carries:
- the four toggle commands: `ToggleFollowCaret`, `ToggleTypedCharacters`, `ToggleTypedWords` and `ToggleReportNewOutput`;
- the three scripts: time, menu and shell list;
- typed text, a control character, a clearing key and an editing chord.

The code reachable in production, with its line counts:

- keys/entry.rs, 67 lines: `run`'s tail, 187-210 (24 lines), and `today()`, 266-325 (60 lines). Of `today()`, two parts are not production code:
  - its two `CaretKey` arms, 286-301 (16 lines), are unreachable from production and from tests alike, since `run` returns first;
  - its `Passed` arm, 323, is reached only from tests, which construct `PressAction::Passed` (say_all_tests.rs 74, tests/replay_validity.rs 727, tests/text.rs 1443).
- reduce.rs, 89 lines: `key_input` 156-176 (21), `command_input` 178-187 (10), `key_trace` 189-200 (12, its `CaretKey` and `HeldKey` arms unreachable), `command_arrived` 248-255 (8), `waited_input` 257-273 (17), `key_arrived` 275-281 (7) and `under_key` 283-296 (14).
  - `reduce_command` (1083-1098, 16 lines) is reached for the four toggles and say-all's stop; no bound gesture reaches its `_ => Vec::new()` arm.
- keys/mod.rs (121 lines): `KeyOrigins` (20-48) is live in production, though not for the bridge's own speech.
  - Speech emitted inside the queue is stamped from the entry's fence (event_queue/driver.rs 1081, entries.rs 617), and `keys::stamp` returns at once for `Input::Queue` (keys/mod.rs 100-102).
  - What `KeyOrigins` stamps in production is speech from inputs outside the queue that come back under a recorded trace. One is `Input::ClockRead` after Report time: main.rs 1611-1612 executes `Effect::ReadClock` under the key's trace, and shell.rs 42 speaks it. The other is `Input::LockKeyToggled` after a lock key, whose trace is recorded through `user_key`'s `ToggleKeyPressed` arm at reduce.rs 58.
  - `user_key`'s other eight arms (50-86) are reached only by tests.
  - Its `KeySeq::NONE_YET` check (84) is reached only by tests and by old dumps ("An input from a flight dump made before inputs carried numbers has none", 50-52), against the recorded rule of no old-dump compatibility.

The code reachable only from tests, with its line counts:

- reduce.rs 104-108, `reduce_input`'s arms for `Command`, `Script`, `CharacterTyped`, `ControlTyped`, `ClearingKey` and `EditingChord` (5 lines). They are reached by terminal/tests.rs, which feeds `Input::Command` and `Input::CharacterTyped` straight into `reduce`.
- The model's `CaretKey` and `HeldKey` variants (event.rs 669-707, 39 lines). Only tests build them, and tests/terminal_support/mod.rs 643-706 (64 lines) converts them back to presses.
- main.rs 1439-1443, the "gesture" log line (5 lines), which matches inputs the shell never applies.
- The model's other six old variants (event.rs 596-638 and 708-766, 102 lines) are built in production only by `today()`.

Change:
- `run` calls `editing::character_typed`, `editing::control_typed`, `editing::clearing_key`, `toggle_setting`, `terminal::toggle` and `shell::script` directly, as the `Own` commands already run.
- `Effect::ReadClock` and `Input::ClockRead`, and `ToggleKeyPressed`'s follow-up `LockKeyToggled`, carry the key number, so `KeyOrigins` can go. Without that change, Report time's and a lock key's speech would lose their key number, which is a speech change.

B's proposal that lock keys join the queue as a press is not part of this finding. shell.rs 110-115 says a lock key "never waits behind a held key, as NVDA starts the announcement's timer when the key is pressed, outside its script queue", so queuing it would change its order.

Net saving: about 370 product lines. B estimated 380; A estimated 150 in the area, leaving out the model's variants and the reduce.rs functions. Tests: the adapter goes (64 lines), and terminal/tests.rs and tests/terminal_reading.rs are rewritten to `PressAction`. Speech is unchanged. A's line numbers for `today()` (389-446) and the record (328) were wrong; B's were right.

### M2. Twelve `Request` implementations of one shape (both)

`grep -n "impl Request for"` over the area gives 12 (21 across Core): caret_key.rs 130, 157, 184; caret_tell.rs 124; changes.rs 246, 278; selection.rs 63, 97; report_focus.rs 62; navigator.rs 76, 110, 142. With their structs they span 359 lines.

- `selection::NodeProperties` (85-121) and `navigator::Properties` (133-166) are the same, character for character, from `type Reply = PropertyValues;` to `_ => None,`.
- `FocusProperties` (report_focus.rs 55-86) differs only in a separate `process` field.
- `ChildCountRead` (changes.rs 272-310) sends the same `Wire::Properties` and picks one answer.
- caret_key's three requests wrap one `OnNode` and differ only in decoding, while `TextRequest` (text_request.rs 17-51) already exists.

Saving: about 200 lines. That is A's figure; B's 250 counts no replacement code. Speech is unchanged.

### M3. Report focus on Core's own focus says nothing (A only; confirmed, with a correction)

Code, report_focus.rs 115 and 126:
```rust
            Committed::Ended(_) | Committed::Own { .. } => Target::Unread,
```
```rust
        Target::Unread => return,
```
Ruling, docs/design/readers-tell-core-asks-build1.md 348, under "### Report focus (Dickson, 2026-11-15)": "A sourceless own focus (the sleeping focus) is spoken from Core's own record, with no request (needs capture of NVDA's speech on the secure desktop; untestable until M8.7)."

They conflict: the code says nothing where the ruling says Core's record is spoken. The commit that wrote it, 42312740 ("Run keys and commands as event-queue entries with report focus reading the committed node"), cites no change of ruling. No test covers `Own` or `Ended` (grep of report_focus_tests.rs; test-coverage.md row 76 lists none). The `Ended` half is a recorded limitation (build1 design 930: "Report focus while the committed focus's reader has ended, before the re-take answers: the node is not there, so nothing is said, where NVDA speaks its object"), so only `Own` conflicts.

But the ruling cannot be built as written today, which A did not see:
- Core keeps no record of the sleeping focus to speak. The only commit of an own focus is input_desktop.rs 99, `state.versions.commit_own(true)`, followed at 102 by `state.focus.sleep()`, which sets the focus object to `None` (focus/object.rs 56-60: "commands find no focus object"). So `Own { source: Some(..) }` is unreachable, and there is no name or role to speak.
- NVDA runs no script at all while its focus is the secure desktop's sleeping object. `inputCore.py` 566-569 raises `NoInputGestureAction` when `focus.sleepMode is focus.SLEEP_FULL`, and the sleeping object is `_SecureDesktopNVDAObject`, with `sleepMode = NVDAObject.SLEEP_FULL` (`winAPI/secureDesktop.py` 54-70).
- In Verbatim, a press reaches this arm only if it was queued before the switch away and runs after it.

Hence a question, not a defect fix (Q1). A's line numbers (164, 175) were wrong.

### M4. Gesture meaning in two tables, a silent fallback, and the command vocabulary written three times (A: tables; B: vocabulary; planned phase M5)

Two tables give a gesture its meaning:
- The hook's map holds membership sets only (map.rs 26-33: `bound`, `observed`, `held`, `held_only`). `bound_gestures` (main.rs 2123-2150) builds it from `bindings_for`, `clearing_keys`, `editing_chords`, `selection_bindings` and `caret_bindings`.
- The app's `Tables::for_layout` (key_reports.rs 44-65) builds a second lookup from the same sources.

In the `Held` arm (key_reports.rs 290-294):
```rust
            let key = tables
                .caret_keys
                .get(&gesture.gesture)
                .copied()
                .unwrap_or(OTHER_KEY);
```
Only a logic bug reaches this fallback, which CLAUDE.md forbids.

The command vocabulary is written three times and dispatched twice:
- `ScriptAction` has 41 variants (scripts.rs 53-97).
- `review_command` maps 39 of them, one to one, onto `ReviewCommand` (147-195).
- `gesture_command` maps again (key_reports.rs 425-435).
- Core then maps `ReviewCommand` to `Own` (entry.rs 238-262), and `Own` to a function (entry.rs 171-182).

Lock keys round-trip through a gesture: the hook reports `gesture: toggle.gesture()` (hook.rs 455-457), and the app looks it up again in `Tables.toggles` (key_reports.rs 56-62).

Phase: M5's "Gestures that change while Verbatim runs" rewrites the map and its lookup (docs/roadmap.md 842-851: "the keyboard hook thread's gesture map published at a version"). B named M4.3, but its entry (roadmap 660-698) has no binding item. The fallback could become an error now, in two lines; the merge of the tables waits for M5. Speech is unchanged.

### M5. The caret key's stage table and loop written twice (both; planned phase M6.4)

`CaretStage::withdrawn_by` (caret_key.rs 308-318) and `TerminalStage::withdrawn_by` (358-374) have identical `Watching` and `Confirming` arms. They differ in two places:
- `Starting { deletes_back } => arrival.counts() && !deletes_back` against `Starting => arrival.counts()`;
- `Sentence => arrival.counts()` against `Final => arrival == Arrival::Key || arrival.counts()`.

`after_pass` (605-640) and `terminal_key`'s loop (716-744) are the same loop over `pending_behind_front`, `confirm` and the watch, ending `if withdrawn && marked(cx, press)`. M6.4 rewrites the terminal half: every terminal feature is redesigned in that phase, "keys owning the changes they cause" among them (roadmap 1271-1272). So this waits. Speech is unchanged.

### M6. Dead model events and toast copies (A for the model; both for the toast)

- A grep of `NormalizedEvent::(SelectionChanged|ControlledSelection|Notification|Alert|ActiveTextPositionChanged)\b` over crates/, xtask/ and uia-facts/ finds only event.rs's own `assign_outpost` match (484-497) and doc links.
- `AlertOrigin` (504-513) has one use outside its definition, the dead `Alert` variant (399).
- `ToastId`'s doc says it is "a hash of its UIA runtime id, computed by the outpost that reports it" (515-518). Core computes it, in toast.rs 90-102 ("FNV-1a over the runtime id's parts"). NVDA compares the runtime id itself, within 1 s (`UIA/__init__.py` 2826-2829).
- `TOAST_WINDOW: &str = "Windows.UI.Core.CoreWindow"` (toast.rs 21) copies verbatim-uia map.rs 425-426, `is_toast_window_class`, which nothing outside map.rs calls (grep: only its own tests, 517-519).
- `node.node = Some(NodeId::new(0));` (toast.rs 69) makes up a node id.

Saving: about 75 model lines (A said 130) and 15 in toast.rs. Speech is unchanged.

### M7. The queue slot's facts are placeholders (B only; confirmed)

- The fence: `fence: Fence::default()` is written at construction in entry.rs 128, announcements/mod.rs 187, changes.rs 741, caret_tell.rs 86, entries.rs 276, 304, 337 and 1038, focus/menus.rs 116, retake.rs 180, 191, 201 and 250, and shown.rs 387. `set_fence` (entries.rs, about 410-430) then sets it again, kind by kind.
- The arrival fact: `AtArrival` is `#[default] OffLine`, "a tell's value from its parse until it joins" (changes.rs 94-96). It is written by default at announcements/mod.rs 139 and changes.rs 742.

Both are facts of the queue's slot, and each can be read before it is set. The change: the slot holds `{ fence, at_arrival, entry }`, set once at the join. About 70 lines, shared with the event-queue area. Speech is unchanged.

### M8. Duplicate types in the input crates (B only, A for `gesture_map_for`; confirmed except `KeyDecision`)

- `ToggleKey::of_gesture` is called only by its own test (toggle.rs 105). `lock_key` and `of_lock_key` are both used (key_reports.rs 61, main.rs 2168), so `ToggleKey` is a second name for the model's `LockKey`.
- verbatim-input-windows `HeldStroke` (lib.rs 143-155) is the model's `Stroke` (stroke.rs 17-28), field for field, converted by verbatim-app strokes.rs 33-40. verbatim-input-windows already depends on verbatim-model.
- `PressAction::Passed` (request.rs 1521) is never posted by the shell (key_reports.rs 343-351 posts `caret_key(OTHER_KEY, None)`); only tests build it.
- `gesture_map_for` (scripts.rs 553-555) has no caller but its test at 934; its own doc says "`verbatim-app` does not call this".

About 70 product and 35 test lines. Speech is unchanged.

### M9. Modifier state kept three times (both)

- The machine keeps `held_modifiers` (state.rs 542).
- The hook tracks Alt again: `alt: (bool, bool)` (hook.rs 58) and `report_alt` (317-330).
- The virtual-key sets are written three times: `reached(&[0x12, 0xA4, 0xA5, 0x5B, 0x5C])` (state.rs 637), `generic_modifier_name` (state.rs 1010-1018) and `Modifiers::after` (typed.rs 296-306). B's citations at state.rs 498-499 and 871-878 are other code.

The change: the `Decision` reports Alt's change from `held_modifiers`, and one modifier table serves all three places. `Modifiers::now()` stays, since it reads Windows' own state for translation. About 40 lines. Speech is unchanged.

### M10. Acceptance re-judged with different flags; the exempt list written twice (B only; confirmed)

- `acceptance::accept` is called at announcements/mod.rs 219, 260 and 314; changes.rs 326 and 650; caret_tell.rs 215; and entries.rs 445, 844, 856, 868 and 891.
- Some calls pass `false`, others `facts.settings.report_background_progress_bars`.
- The `Accepted` list at announcements/mod.rs 260-268 repeats entries.rs 446-454, `accepted_apart_from_foreground`.
- `msaa_passes` (mod.rs 318-326) repeats changes.rs `passes` (324-335).

Removing the copies saves about 30 lines, with no speech change. Judging once, at the join, would change which value of the setting applies when it is toggled while an event waits. entries.rs 436-442 records today's choice: "The setting is judged as it stands at the foreground change". That part is Q5.

### M11. A missing property answer taken as "no states" (both; defect)

changes.rs 207-210:
```rust
            Self::States => match value {
                Some(PropertyValue::States(states)) => PropertyChange::States(*states),
                None => PropertyChange::States(StateSet::default()),
```
`value` comes from `.answer(changed.property()).and_then(|answer| answer.value.as_ref())` (460-461). It is `None` both when the reply omits the property, a reader's fault, and when the object has no value. `answers_in_order` and `SetMismatch`, written to check a reply against the set asked, have no caller outside verbatim-model (grep count 0).

Sequence:
1. The focus is a checked check box.
2. An MSAA state change arrives.
3. The outpost's reply omits `States`.
4. Core judges an empty set against the states last spoken, and speaks the change as if every state were removed.

Only a reader fault reaches this. The fix: decode `Properties` replies through `answers_in_order` once, in M2's request, and add one test. Speech changes only under a reader fault.

### M12. The navigator's window as a patched `Option` (both)

- state.rs 152-156 documents the window as "`None` for an object the navigator was put on otherwise".
- navigator.rs 10-13 says: "An object with neither, put there by the old focus path, is read by no command until that path is deleted". Commit 3f9e68ac ("Remove Core's old focus paths and judge every focus through the event queue") removed that path, so the comment is stale.
- changes.rs 517 writes `navigator.window = Some(window);` after `Navigator::on`, and five more sites build a navigator with no window (reduce.rs 382, 456, 465 and 1411; navigator.rs 415).

The change: `Navigator::on(object, window)`. About 25 lines. Speech is unchanged.

### M13. `Decision` as eight fields (A only; confirmed)

state.rs 234-273 has eight fields: `verdict`, `emitted`, `speech`, `shared_modifier`, `observed`, `typing`, `toggle` and `mask`. Their docs state the invariants: `emitted` is "Only ever set on a [`Verdict::Swallow`]", and `observed` is "Never set on a [`Verdict::Swallow`]". Variants that carry what each can report would make the wrong combinations unrepresentable. Lines about even. Speech is unchanged.

### M14. Smaller copies (details: both; the rest B only, confirmed)

- `TextReason` has the single variant `CaretKey` (details.rs 69-73).
- `focus_set` (26-37) and `walk_set` (48-59) repeat `node_set`'s filter with one exception each.
- Each announcement function re-matches the kind `Announcement::run` already matched, with an `else` that only a logic bug reaches. Example, toast.rs 28: `let Kind::Toast(kept) = &announcement.kind else {`. The same holds in alert, show, desktop_name, selection and notification.
- `OutpostId(instance.generation)` is written 9 times in Core's product code (grep), once each in alert, selection, caret_tell, changes, entries, msaa_run, retake, uia_focus and caret_key.

About 60 lines. Speech is unchanged.

### M15. A stale parity entry and stale documents (B only; confirmed)

docs/parity/input.md 30-42 says a command pressed while a caret key is held "runs after that key has been passed on and its watch answered, or 100 ms after the pass". It cites the test `a_review_command_pressed_while_a_caret_key_is_held_runs_after_its_read_pass_and_watch`, and grep finds no such test. key-holding.md 111 says the opposite: "Verbatim's own commands pressed behind a caret key mark it silent and run at their turn, at once, so a reading command may read the line before the key's move, as NVDA's capture does". The parity entry's last sentence, "Verbatim says "line two" in both cases", is therefore wrong for the back-to-back case.

Also stale:
- the crate guide's account of keys turned into `Input::CaretKey` and `Input::HeldKey` (verbatim-input.md 142-144);
- its account of `gesture_map_for` (84-96);
- verbatim-input-windows lib.rs 235 and typed.rs 1, which name `Input::CharacterTyped`.

Documents only.

### M16. Auto-repeat not counted as a multi-press (B only; confirmed, quotation corrected)

state.rs 107-108: "NVDA counts auto-repeat as presses; this is a deliberate difference (`docs/parity/input.md`, "Script repeat counting")". B's quotation and line (772-773) were not in the code. docs/parity/input.md 27-29 says the same. No ruling of Dickson's is cited: grep over docs/ and books/ for "auto-repeat" with "Dickson" finds none. NVDA counts any run of the same script within the timeout (`scriptHandler.py` 286-290), and ignores repeats only for the pause-speech key (`keyboardHandler.py` 299). This is a difference in speech with no ruling: Q2.

### M17. The MSAA alert path (A only; confirmed)

alert.rs 11-12: "No application is known to reach this path; its differences from NVDA are recorded as unverified gaps in the parity entry." Its cost:
- 140 product lines;
- the `moved: bool` requeue flag (announcements/mod.rs 75-76);
- four tests (announcements/tests.rs 897-1038, 141 lines).

It is NVDA's behaviour (IAccessible `event_alert`), so it is a feature: Q3.

### M18. Refused posts kept and re-posted (both)

key_reports.rs 13: "A post a port refuses, which only a system out of memory does, is kept". `PostSubject` (79), `KEPT` (88), `post` (112) and `try_post` (132) make about 95 lines, with three tests of about 120 lines, from 15b2b07a ("Run the failure paths through fault points"). A refused post is retried only when the next key comes. A question of cost against value: Q4.

### M19. Terminal copies (planned phase M6.4)

- keys/terminal.rs 49-87 keeps `state.terminal_focus.deferred` for the keys behind a held stroke, which the stroke line already holds; `released()` empties it (84) once `strokes.holds()` is false (51). A cited 483-521, past the file's 93 lines.
- caret_tell.rs 207-208, `dropped_in_console` ("Verbatim-only: the terminal model"), drops a console's caret tell at its arrival and again at its turn.

Both wait for M6.4.

## 3. Test findings

verbatim-core:
- Removable now (about 25 lines):
  - navigator_tests.rs `with_no_navigator_a_command_says_so` (363-369, Activate alone) is a subset of `with_no_navigator_each_navigator_command_says_so` (496-509, which loops over Activate).
  - The first half of `a_gone_object_says_the_edge_and_an_ended_reader_nothing` (120-129, NextSibling not there) repeats one case of `every_direction_from_an_object_not_there_says_its_edge` (432-449).
- Removable with M1: tests/terminal_support/mod.rs 643-706, the adapter (64 lines). terminal/tests.rs's direct `Input::Command` and `Input::CharacterTyped` inputs, and tests/terminal_reading.rs's 13 old-variant sites, are rewritten to `PressAction`.
- Replaceable by captured expectations: 6 tests say "Needs capture: written to the design's derived speech". A's count was right; B's 11 was not.
  - caret_key_tests.rs: `backspace_with_a_focus_pending_says_its_deleted_text_then_the_focus_is_taken`, `backspace_with_a_focus_pending_at_its_starting_read_reads_then_says_its_deleted_text`, `a_key_whose_field_is_no_longer_the_focus_runs_on_that_field` and `a_later_key_during_a_sentence_keys_watch_still_moves_it_silently`.
  - caret_tell_tests.rs: `a_caret_tell_with_a_focus_pending_asks_nothing` and `caret_tells_waiting_for_one_element_merge_into_one_read`.
- Merged with M5, at M6.4: `every_cell_of_the_edit_paths_stage_table` and `every_cell_of_the_terminal_paths_stage_table`, about 40 lines.
- If Dickson removes the alert path (Q3): 4 tests, 141 lines.
- To add: one test of a `Properties` reply missing an asked property (M11); one of report focus over Core's own focus, once Q1 is ruled.
- Totals: about 90 lines removable now, 6 tests to re-expect from captures, 2 to add.

verbatim-input:
- Removable: six scripts.rs tests, about 220 lines (B's count).
  - `desktop_table_binds_exactly_the_documented_gestures` (619) and `laptop_table_binds_exactly_the_documented_gestures` (641) retype the tables.
  - `every_m4_command_is_bound_in_both_layouts` (706), `report_status_bar_is_bound_in_each_layout` (758), `both_layouts_bind_time_tray_list_and_report_focus_identically` (880) and `the_laptop_layout_keeps_the_numpad_bindings` (901) restate parts of them.
- Removable with M8: `gesture_map_for_contains_exactly_the_bound_gestures` (934) and toggle.rs's `of_gesture` test, about 25 lines.
- A question (Q6): tests/hold_sequences.rs (716 lines, generated) breaks two of CLAUDE.md's rules for generated tests. Its model half is about 250 lines.
  - Its model takes each press's meaning from the machine. Module doc, 35-36: "The model states, for a press whose meaning it takes from the machine"; at 463-475 the meaning is read off `decision.emitted` and `decision.verdict == Verdict::Swallow`.
  - It exempts a recorded limitation (19-23): "The permit never falls to `None` while a deferred press or a mask waits in line".

verbatim-input-windows:
- Removable: hook.rs `a_burst_larger_than_the_old_bound_is_delivered_in_order_with_none_lost` (492-540, about 50 lines). Its sink is a `Vec`, and the bound it names went with 0d1e7802 ("Send every gesture from the keyboard hook on an unbounded channel").
- Removable with M9: the hook's Alt tests, about 40 lines.

verbatim-app (key_reports.rs):
- With M4, at M5: about 120 lines.
  - `each_gesture_maps_to_its_script_command_or_lock_key` (587) and `a_laptop_gesture_maps_to_its_command_only_in_the_laptop_layout` (631) become one test of the binding report.
  - `each_report_maps_to_its_commands` (524), `a_held_enter_maps_to_the_new_line_key` (837) and `a_held_caret_key_maps_to_its_held_key` (868) shrink.
- If Dickson removes the retry (Q4): three tests, about 120 lines.

## 4. Questions for Dickson

Q1. Report focus pressed before a switch to the secure desktop and run after it (M3).
- The decision: the ruling of 2026-11-15 (build1 design 348), "A sourceless own focus (the sleeping focus) is spoken from Core's own record, with no request".
- The facts that changed since:
  - Core's record of the sleeping focus is empty, because `focus.sleep()` sets the focus object to `None` (input_desktop.rs 102; focus/object.rs 56-60).
  - NVDA runs no script while its focus is the sleeping object (`inputCore.py` 566-569; `winAPI/secureDesktop.py` 54-70).
- Sequence: Notepad focused; Verbatim+Tab; the UAC prompt opens before the press's turn.
- Today: nothing is said.
- Option A: the ruling is replaced by "says nothing", keeping today's code; one unit test pins it.
- Option B: Core keeps a record of the sleeping focus and speaks it. Its speech needs a capture of NVDA on the secure desktop, untestable end to end until M8.7, as the ruling says.

Q2. Auto-repeat and multi-press (M16).
- Sequence: hold Verbatim+numpad 5 past the auto-repeat delay.
- Verbatim today: the object is reported at every repeat, each with repeat count 0 (state.rs 104-108).
- NVDA: counts each repeat as a further press of the same script (`scriptHandler.py` 286-290); its speech needs a capture.
- Keep the difference as a ruling, or match NVDA.

Q3. Keep the MSAA alert path that no known application reaches (alert.rs 11-12), or remove it until one does (M17).
- It costs 140 product and 141 test lines, and the `moved` flag.
- Removing it changes speech only for an application raising an MSAA alert; none is known.

Q4. Keep the hook's re-posting of refused posts (M18), or drop it and log the refusal.
- It costs about 95 product and 120 test lines (key_reports.rs 13 and 77-170; 15b2b07a).
- It covers a refusal "only a system out of memory" causes.
- No speech changes in any known sequence.

Q5. Judge acceptance once, when a tell joins the queue, rather than at each stage (M10).
- Sequence: a background progress bar's value change waits in the queue; the user turns "report background progress bars" off; then the foreground changes.
- Today: the change is stopped, the setting judged as it stands at the foreground change (entries.rs 436-442).
- Judged once at the join: the change is spoken.
- NVDA's speech in that sequence needs a capture; entries.rs says NVDA "stops nothing at a foreground change".

Q6. hold_sequences.rs, the generated walk of the hook's holding. Choose one:
- give its model its own gesture table and drop the exemption, after which it fails on the limitation recorded in key-holding.md, "Limitations";
- or delete its model half (about 250 lines), keeping the checks that need no model.

## 5. Cost of one behaviour

From `git show --stat`:
- 7431d850, holding Enter in text fields: 35 files, +1,294/-229. NVDA's `script_caret_newLine` is editableText.py 208-239, 32 lines (A's citation; B's 191-216 is wrong).
  - verbatim-input: state.rs; map.rs, its new `held_only`; scripts.rs.
  - verbatim-input-windows: hook.rs, typed.rs, lib.rs.
  - The app: key_reports.rs; main.rs, `bound_gestures`.
  - The model: event.rs, request.rs, text.rs.
  - Core: caret_key.rs, editing.rs, entry.rs, keys/mod.rs, keys/terminal.rs, reduce.rs, state.rs, text.rs.
  - Tests: 386 lines of caret_key_tests.rs.
  - Follow-ups the same day: ca937fed (8 files, +164) and 0729e1b9 (14 files, +128).
- 22481fe2, the task switcher by Core's Alt state: 20 files, +487/-61, including the hook's Alt tracker (M9).
- 7aba6761, state changes judged by the focus line at their arrival: 21 files, +1,677/-254.
- e380a0ce, Control+Backspace saying the whole word: 28 files, +633/-159.
- f75ad9f4, Alt+Up or Down in UI Automation text: 5 files, +133/-10, the shape a behaviour should have.

A new command touches:
- `ReviewCommand`, `ScriptAction` and `review_command`;
- both layout tables and the tests that restate them;
- `gesture_command`, `Own`, `Own::of` and the `run` match;
- its function, and usually a new `Request` of about 30 lines.

A new caret-key behaviour touches:
- the binding table, `bound_gestures` and `Tables`;
- the model's press and old `Input` variants;
- both stage tables and both loops in caret_key.rs;
- editing.rs, and often keys/terminal.rs.

Shared mechanisms that cut it, merged:
- One binding per gesture, naming the model's command and carried from the hook to Core (M4, at M5), removes `Tables`, `ScriptAction`'s mirror and the filters in main.rs.
- Presses run as their entry's own functions (M1) remove the old `Input` variants and the bridge: a new kind of press is one `PressAction` arm and one function.
- One generic outpost request with one properties request (M2) removes the `Request` type each behaviour adds, and validates replies once (M11).
- The fence and arrival facts on the queue slot (M7) keep each new entry kind from carrying them.
- One stage table and one loop (M5, at M6.4) halve caret_key.rs's edits.

What remains follows the process layout (D9, D13, ADR 0003), which this area cannot remove.

## 6. Dropped

- B: `KeyDecision` "duplicates `Verdict`". It is the callback's two-valued answer to Windows, projected from `Verdict`'s four, not a copy.
- B, F11: the two representations of the focus (caret_key.rs `binding` 80-99 defaulting to `(None, TextAttributes::default())`; navigator.rs `to_focus`). The quotation is right, but no sequence in this area reaches a mismatch; it belongs to the focus pair's review.
- B, part of F1: "Lock keys join the queue as a press". It would change the lock key's order (shell.rs 110-115).
- A: the three state-change tests that "should be replaced by recorded orders". They are in changes_tests.rs, not announcements/tests.rs, and the two recordings replay_announcements.rs replays (`ann_button_win32_run1`, `ann_button_wpf_run1`) are of an Apply button disabling itself, not of these orders.
- A: that hold_sequences.rs "overlaps about 16 hand-written tests, about 500 lines". The overlap was not established.
- B: that caret_key_tests.rs `a_deferred_transition_goes_with_the_held_key_ahead_of_it_and_at_once_with_none` and `at_exit_core_names_the_held_strokes_for_the_shell_to_release` repeat strokes.rs unit tests. They run through the reducer's routing, which the strokes.rs tests do not, so they are not duplicates.
- Corrected and kept, not dropped:
  - A's model saving (130 lines, now about 75);
  - B's count of estimated expectations (11, now 6);
  - B's phase M4.3 (now M5);
  - B's NVDA line range for the new-line script;
  - B's modifier line numbers;
  - A's line numbers in entry.rs, report_focus.rs and keys/terminal.rs.
