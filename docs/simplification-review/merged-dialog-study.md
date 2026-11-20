# A dialog read once it is shown: the two studies checked, and the options

Read-only comparison of study A (`dialog-study-A.md`) and study B (`dialog-study-B.md`) against main at ba89dca7, the build bc99bc40 to 870bda79, ADR 0003 and `books/dev/src/explanation/design/focus.md`, "A dialog read once it is shown". NVDA claims are from the pinned submodule, 732d1ce2. Nothing was built or run.

Terms used here. "The wait" is today's mechanism: the outpost answers `Reply::HiddenDialog`, Core asks the listener `Request::AwaitShown`, and the queue's front holds until the listener answers. "Postponement" is study A's: the read that meets a hidden dialog ends its entry, Core keeps one fact (the postponed dialog's window), and the listener's tell of the window's show joins the queue as an ordinary event whose turn reads the focus again. "Parked read" is study B's B1: the outpost holds the read in its waiting table, polling the window's visibility about every 10 ms, and runs it once visible or at a bound Core names (proposed 300 ms). "Inline wait" is study B's B0: the outpost's worker loops on visibility inside the read. "Fenced" speech is speech carrying a key number lower than the newest key whose press cut speech; the speech manager drops it unspoken (`crates/verbatim-speech/src/manager.rs` 963 to 968).

## Decisions for Dickson, first

1. Which shape replaces the wait, if any: today's; postponement (A); parked read (B1); inline wait (B0). The facts below, and my reading at the end.
2. For a parked or inline wait, the bound (a performance budget, his): it decides speech for any dialog shown later than it.
3. The speech differences each option brings (section 3), each a "what users hear" decision. The ones that differ from today: a modifier pressed alone while hidden (A speaks the dialog with its text at the show; today and B are silent); a key's dialog speech under A unless its read is stamped with the front's older fence (A as written speaks it; today and B fence it off); a tray menu over a hidden dialog-based window and a focus elsewhere that executes (B speaks the dialog first, against the ruled option C); a background application's speaking entry (A speaks it before the dialog); a hang while hidden (A plays no sound until something reads the window); a dialog never shown (A: nothing, nothing held; B: the dialog without its text at the bound; today: nothing, 13 s held); a dialog shown after the bound (B says it without its text); a caret key pressed while hidden (B can hold it past the 150 ms release, after which it says nothing).
4. For A, whether the listener tells every window object's show or only the foreground window's (a new local drop in a reader, not one of NVDA's), after the count in section 5 is measured.

## 1. The claims each study rests on

### Study A

- Recovery of a hung window works as "postpone and re-take", holding nothing. Confirmed. `crates/verbatim-core/src/event_queue/table.rs` 160 to 163 keeps `recovery: BTreeMap<u64, RequestWindow>`, "watched until they answer again"; `event_queue/inbox.rs` 491 to 497 (`hung`: sound and `watch_if_attended`); `entries.rs` 703 to 709 (`recovered` joins `Entry::CurrentFocus`); `focus/retake.rs` 196 to 203 (`Why::Recovered`, `fence: Fence::default()`). Nothing holds the queue. Caveat: its release is Core's own half-second live check, not a reader's tell, and it reads the application's current focus, not the read that was stopped.
- The return from another input desktop works the same way. Partly. `crates/verbatim-core/src/input_desktop.rs` 118 to 141: a `ReturnPending { due_us }` and a tick that returns `retake::returned`. It is a read scheduled by a timer after the switch, not a read that met a condition and was postponed; it shows the re-take entry, not the postponement.
- The listener's hook already hears every window object's show. Confirmed. `crates/verbatim-ia2/src/hook.rs` 198 subscribes `EVENT_OBJECT_SHOW`; 1035 to 1042 hand every show on (`OBJID_WINDOW`, `CHILDID_SELF`) to the second output before any filter; 930 to 938 and 1000 to 1006 keep only the caret, NVDA's other show classes and the tooltip as tells.
- What telling every window object's show (or only the foreground window's) would cost in tells. Not checkable: no measurement in the repository counts window-object shows. `books/dev/src/reference/performance.md` 624 to 631 says only that each show costs the listener "one comparison of its ids" today, and 345 to 362 that a tooltip's show tell costs 19 local calls to Windows for its facts. A tell of every window-object show would pay a tell's facts, a pipe message and a wake of Core per show; the rate is unknown. Section 5 says what would settle it.
- What a key at its turn would have to do. Confirmed in shape, wrong in one consequence. The key must read the focus before it runs (`a_key_during_the_wait_reads_the_dialog_at_once_then_is_handled`, `crates/verbatim-core/src/focus/shown_tests.rs` 866 to 878). A says the speech is "the same, by the key rule". It is not, as written: today the dialog's speech is said by the front focus entry and carries that entry's fence, below the key, so the key's cut silences it (`KEYED_SPOKEN`, shown_tests.rs 343 to 350, comment "fenced below the key, so its cut silences them"; `crates/verbatim-core/src/entries.rs` 616 to 617 stamps every utterance with the front entry's fence). A read made inside the key's own entry is stamped with the key's own number (`keys/entry.rs` 136 to 143), which is not below the cut, so the dialog and field become audible before the echo. The rule must stamp the read with the postponed entry's older fence to keep today's speech.
- "A modifier alone joins no queue entry". Confirmed (`keys/entry.rs` 51 to 55: "a modifier, which joins no queue"). But A's account of today's speech for it ("the dialog is read at once without its text") omits that the read's speech is fenced by the modifier's cut, so today the user hears nothing for the dialog (`a_modifier_alone_ends_the_wait_as_its_press_cuts_speech`, shown_tests.rs 1176 to 1203, asserts the read; the fence does the rest). Under A the show's re-take is a new entry; if it takes `Fence::default()` like every re-take today (`retake.rs` 186 to 203) it is never fenced, so the dialog is said with its text after the modifier. The difference A flags is real but is "silent today, spoken under A", not "spoken hidden today, spoken shown under A".
- The queue hooks serve only the wait. Confirmed: `own_foreground_rule`, `stops`, `merged`, `answer_cancelling`, `stopped_at_once`, `ended_wait` appear outside tests only in `entries.rs`, `event_queue/` (driver, inbox, limiter, mod, outposts, table), `focus/msaa_run.rs`, `focus/uia_focus.rs` and `focus/shown.rs` (grep of `crates/verbatim-core/src`).
- `Foreground::holds` does not stop the wait for the owner or an owned box. Confirmed: `event_queue/mod.rs` 165 to 172 compares the request window's top-level window and root owner with the foreground window and its root owner, so a box or the owner sharing the dialog's root owner holds.
- No end-to-end scenario uses the stand-in. Confirmed: no scenario under `crates/verbatim-e2e/src` names `late_dialog` or its templates; `books/dev/src/reference/test-coverage.md` 122 lists only `quit_inside_a_file_dialog`.
- The replay test finds its place by a substring match. Confirmed: `crates/verbatim-core/tests/replay_validity.rs` 1119, `label.contains("Import a Theme")`, which the testing standard forbids.
- The present fixture cannot be replayed under a new protocol. Confirmed: `crates/verbatim-core/tests/fixtures/dialog_shown_import.jsonl` carries one `HiddenDialog` and one `"Shown"`. True for every rebuild, B's included.
- "Today 325 to 484 ms from Enter on Import". Confirmed, `performance.md` 610 to 621.
- NVDA's caret wait (`editableText.py` 72). Confirmed: "Waits for the caret to move, for a timeout to elapse, or for a new focus event or script to be queued."
- Line counts. Confirmed within a few per cent by `git diff --numstat bc99bc40^ 870bda79 -- crates`: shown.rs 394, shown_tests.rs 1,247, listener/shown.rs 255, listener-logic/shown.rs 239 (136 product), listener.rs +343, hook.rs +57, event_thread.rs +142, asked.rs +82, request.rs +49, the event queue's files about 250, late_dialog.rs 359.

### Study B

- The outpost's waiting table and the caret watch work as it says. Confirmed: `crates/verbatim-outpost-logic/src/watch.rs` 1 to 33 (polled at `poll_us` from the pass until `bound_us`, both from Core's `Watch`, `crates/verbatim-model/src/text.rs` 815 to 833), with cancel (watch.rs 330), destroyed (349) and close (377).
- It could hold one more kind. Plausible but larger than B says. The watch's table is specific to caret watches (`Watching` carries a node, a `Watch` and three phases; about 414 product lines before the tests at 415). A parked read needs its own kind or an enum over kinds, a check that names the dialog's root rather than the request's ids, and a path back to the worker's queue. Note that the queued requests' own "destroyed" path matches only a request's MSAA ids (`crates/verbatim-outpost-logic/src/waiting.rs` 33 to 55 and 104 to 125): a current-focus read or a UI Automation completion is never answered "not there" by a window's destruction today, so the parked kind needs its own match on its root. My estimate for the table kind is 100 to 150 lines, not 75.
- Core's validity rows already refuse a focus not in front when a read returns (`validity.rs` about 801 to 804). Partly. The row is `if view.in_front != Some(ids.window) { return Ok(Settled::Refused(Reason::NotInFront)); }` at `crates/verbatim-core/src/focus/validity.rs` 800 to 804, inside `if tell.kind == EventKind::Foreground`: it refuses a foreground event's candidate only, never an MSAA focus, a UI Automation focus or a current-focus re-take. `view.in_front` is Core's foreground record (validity.rs 646 to 648), and the row is judged again on the answer (`read_at`, `focus/msaa_run.rs` 412). So the dialog's own foreground read is refused when the box or the owner is in front; a field focus inside the dialog is refused only by another row (its focused state), which no test checks for this sequence.
- "The present design duplicates this row with its own foreground leg". Partly: the outcome for the dialog's foreground candidate is the same, but the present ending acts at the foreground change and the row only when the read returns, up to the bound later.
- A key's press already fences off and cuts the dialog's speech (`KEYED_SPOKEN`). Confirmed, with a caveat. The fence is the front entry's (`entries.rs` 616 to 617), and the speech manager drops speech whose key is below the newest cut (`manager.rs` 963 to 968). In the test the front's fence is `None` because no key preceded it, and `None` is never dropped (`key.is_some_and(...)`); in a real run the Enter that opened the dialog gives it a number, so it is dropped. The audible speech is therefore the key's own in today's design and in B's. Not considered by B: a caret key is held by the hook only 150 ms from its press (`crates/verbatim-core/src/keys/caret_key.rs` 15 and 502 to 511) and "a caret key released by that timer says nothing" (focus.md, "Differences from NVDA"), so a caret key held behind a parked read for longer than 150 ms says nothing, where today the key ends the wait at once.
- The measured delays between a dialog's foreground and its show, 14 to 221 ms. Confirmed: `performance.md` 603 to 608 (recordings of 2026-11-19: Notepad's Save As 15 to 32 ms, Import a Theme 93 to 221 ms, a message box 16 ms, the Run dialog 17 to 22 ms when hidden, Explorer's Properties 14 to 30 ms) and focus.md 100. Five dialogs, each a few runs, on one machine; no slow machine, no cold start, no runner.
- "NVDA flushes the waiting foreground before a menu start in the same pass (`IAccessibleHandler/__init__.py` 1061 to 1092) and accepts a foreground on the window in front (816 to 841)". Confirmed: a non-focus event flushes the queued focus and foreground events first (1061 to 1080) and a menu start is kept as the "fake focus" and processed after with `validFocus` (1093 to 1099; `processMenuStartWinEvent` 942 to 973 returns early only when the last queued focus is a menu). B's conclusion "B1 is NVDA's order" is derived: NVDA's `doPreGainFocus` drops the expired focus's queued speech when the menu's focus executes (`eventHandler.py` 388 to 400), so what NVDA lets the user hear of the dialog is not known without a capture.
- "Two fix commits already in the mechanism after its review, one short of the stopping rule". Matter of counting: bb65a26b fixes the review's findings and 8c3978c7 fixes the stand-in and the record of shows out; whether bb65a26b counts as a fix round is Dickson's reading of the rule.
- "Hung: the same" under B1. Confirmed in shape: the parked read is an ordinary request out, so Core's half-second live check (focus.md, "Slow and hung applications") stops it as hung and plays the sound, as `a_dialog_hung_during_the_wait_is_not_responding` (shown_tests.rs 975 to 993) expects today.
- Line counts. Confirmed as for A.

## 2. The four options in each sequence

Speech today is from the tests. `DIALOG_SPOKEN` (shown_tests.rs 376 to 379): `Label("Import a Theme") Role(Dialog) Description("File name:")`, `Label("File name:") Role(EditableText)`. `DIALOG_SPOKEN_HIDDEN` (542 to 545): the same without the description. The real Import, `crates/verbatim-e2e/src/scenarios/quit_inside_a_file_dialog.rs` 46 to 53: "Import a Theme dialog File name:", "File name: combo box collapsed", "edit Alt+n", "blank" (NVDA's capture). Speech under the rebuilds is derived from the code; none is stated by a test or capture unless said.

NVDA in every sequence: it never waits for a show. A foreground event is processed only if its window is still `GetForegroundWindow()` at its pass (`nvda/source/IAccessibleHandler/__init__.py` 834 to 841); a dialog's text skips invisible children (`nvda/source/NVDAObjects/behaviors.py` 104 to 107), so a dialog read before its show has no text. NVDA said Import's text only because its pass ran late, 716 to 1021 ms after Enter (`performance.md` 608 to 609; `docs/parity/focus-context.md`, "A dialog's text").

### The plain show

- Today: the read answers hidden, the listener is asked, the show answers, the read again speaks `DIALOG_SPOKEN` (`a_hidden_dialog_is_read_again_once_shown_and_speaks_its_text_then_its_field`, 430; `the_field_behind_the_dialog_is_said_once_with_it`, 551; the scenario). Added silence: the time to the show plus one listener round trip.
- A: the foreground's read postpones, the field's read postpones too, the show's tell joins and its re-take reads the current focus with `Check`: `DIALOG_SPOKEN`, derived. Delay: the show plus one tell.
- B1: the read parks, runs at the first poll that finds the dialog visible: `DIALOG_SPOKEN`, derived. Delay: the show plus up to 10 ms.
- B0: the same as B1.
- NVDA: the dialog said with its text only if its pass reads after the show (Import, captured); for the Run dialog (17 to 22 ms) no capture says.

### A dialog shown at its foreground

- All four: no wait, `DIALOG_SPOKEN` (`a_shown_dialog_asks_nothing_of_the_listener`, 1065). A's show tell, if any, finds nothing postponed.
- NVDA: the same speech.

### A key pressed while hidden

Typed character (`a_key_during_the_wait_reads_the_dialog_at_once_then_is_handled`, 866; `a_key_before_the_wait_...`, 881):

- Today: the key ends the wait; the focus is read at once, hidden, and committed: `DIALOG_SPOKEN_HIDDEN` fenced below the key, so unspoken in a real run; then `Character("a")`. Heard: "a".
- A as written: the key's entry reads the focus, then echoes: `DIALOG_SPOKEN_HIDDEN` stamped with the key's own number, so heard, then "a". Heard: the dialog without its text, the field, "a". With the read stamped with the postponed entry's fence: "a" only, as today. No test states either.
- B1: the key waits behind the parked read until the show or the bound; the read's speech, `DIALOG_SPOKEN` or `DIALOG_SPOKEN_HIDDEN`, carries the front's fence and is dropped; then "a". Heard: "a", up to the bound later.
- B0: the same as B1.
- NVDA: the cut is queued on the event queue behind events already queued (`nvda/source/inputCore.py` 584 to 604), so a dialog whose gainFocus was queued first is cut; whether NVDA had queued it before the press depends on its pump. Not captured.

Caret key (no test in `shown_tests.rs`):

- Today: the key ends the wait at once; after the hidden read the caret key's own turn reads and speaks its line if it comes within 150 ms of the press, otherwise it was released and says nothing. The hidden read's time is not measured.
- A: as today, plus the fence question above.
- B1 and B0: the key waits for the show or the bound; past 150 ms from its press it is released and says nothing. For Import (93 to 221 ms to the show) that is often. Derived.
- NVDA: its caret script runs after the queued gainFocus, waiting for the caret (`editableText.py` 72). Not captured.

Modifier alone (`a_modifier_alone_ends_the_wait_as_its_press_cuts_speech`, 1176):

- Today: the wait ends, the hidden read is committed, its speech fenced by the modifier's cut. Heard: nothing for the dialog, then or after.
- A: nothing is read at the press; at the show the re-take speaks `DIALOG_SPOKEN`, unfenced. Heard: the dialog with its text, after the show.
- B1 and B0: the parked read runs at the show; its speech carries the front's fence and is dropped. Heard: nothing, as today.
- NVDA: the modifier queues a cut; its result depends on the pump as above. Not captured.

Tab (`tab_during_the_wait_is_judged_from_the_field`, 897): today the hidden field is committed first and Tab is judged from it. Under all four the Tab passes to the application at its press and the application moves the focus; a read made after that move commits the new control under the old fence, and the new control's own focus event is then a duplicate, so nothing is said for it. The test answers the read with the field, so it does not show this. The window for it is one read today and under A, and the time to the show under B. A capture with Tab pressed during a hidden dialog would settle it.

### The foreground moving to another application while hidden

- Today: `"cancel Listener LeftHiddenDialog"`, nothing said (`the_foreground_moving_to_another_application_ends_the_wait_as_elsewhere`, 1136).
- A: nothing is out; the postponement stays, the gate is closed while another window is in front, and the other application's focus commit clears it. Nothing said for the dialog.
- B1: the generic foreground stop (`inbox.rs` 609 to 613) cancels the parked read at once; the table answers it cancelled. Nothing said.
- B0: the same stop; the worker is inside its loop, so it is abandoned and replaced (ADR 0003, "Who holds what"). Nothing said.
- NVDA: the dialog's foreground is dropped if the other window is already in front at its pass (834 to 841); otherwise the dialog is said without its text, then the other application. Not captured.

### A message box owned by the hidden dialog

- Today: the wait ends at the box's foreground; `Label("Replace it?") Role(Dialog)`; after OK, `DIALOG_SPOKEN` (`message_box_then_dialog`, 630 to 649, in both orders).
- A: the dialog is postponed; the box is read and spoken at its turn, its commit clears the postponement; after OK the dialog's foreground read finds it shown (or postpones until its show): the same speech, derived.
- B1: the box is said after the dialog's parked read returns at the bound and is refused as not in front (validity.rs 800 to 804); a field focus inside the dialog then parks again for up to another bound and is refused by its focused state; the same words, up to one or two bounds late. Derived.
- B0: the same as B1.
- NVDA: the dialog's foreground is dropped if the box is in front at its pass; otherwise the dialog without its text, then the box. Not captured.

### A dialog closed by hiding

- Today: `Label("Export...") Role(Button)` (`owners_focus_spoken`, 669 to 677).
- A: the owner's focus commits at its turn: the same.
- B1 and B0: the dialog's read waits to the bound, is refused as not in front, then the owner's focus: the same words, a bound late.
- NVDA: as for the box.

### A tray menu opened over a hidden dialog window

- Today: `Label("Context") Role(Menu)` only (`a_menu_start_during_the_wait_ends_it_and_the_menu_is_spoken`, 697; `..._before_the_wait_...`, 716).
- A: the dialog is postponed, the menu start is read and committed at its turn: the same.
- B1 and B0: the menu waits behind the parked read; the window is never shown, so at the bound the dialog is read hidden and, still in front, committed: `DIALOG_SPOKEN_HIDDEN` (and its focus), then the menu. Whether Verbatim's own expired-focus drop then silences the dialog's speech is not stated by any test. Differs from today and from the ruled option C ("says nothing for the hidden dialog", `docs/parity/focus-context.md`).
- NVDA: the foreground is processed before the menu start in the same pass, then the menu as a forced focus (1061 to 1099, 942 to 973); the dialog's queued speech may be dropped as an expired focus (`eventHandler.py` 388 to 400). Not captured; the stand-in's tray variant waits for its recording.

### A focus elsewhere in the same application

- Refused by its read (`an_ending_focus_refused_by_its_read_reads_the_dialog_again_once_shown`, 775): today `DIALOG_SPOKEN` after a re-take; A the same, from the show's re-take; B1 and B0 `DIALOG_SPOKEN` from the first read once shown, then the focus refused.
- That executes (`an_ending_run_whose_older_candidate_executes_reads_nothing_again`, 799): today `Label("Rename...") Role(Button)` only; A the same; B1 and B0 the dialog first (with or without its text, by the show and the bound), then "Rename... button". Differs, against option C.
- A background application's menu or focus (`a_background_menu_start_leaves_the_wait`, 736; `a_background_focus_refused_by_acceptance_leaves_the_wait`, 753): today and B, the dialog at its show, then the background entry; A, the background entry judged first. These two tests' background entries say nothing, so the order shows only for one that speaks.

### A dialog never shown

- Today: the queue held 13 s, then `"cancel Listener DueTime"`, nothing said (`a_dialog_never_shown_ends_at_the_due_time_unspoken`, 962).
- A: nothing said, nothing held; a later key reads it hidden.
- B1 and B0: at the bound, `DIALOG_SPOKEN_HIDDEN` and its focus; keys held for the bound.
- NVDA: the dialog without its text and its focus, as the events come.

### A dialog destroyed while hidden

- Today: not there, nothing said (`a_dialog_destroyed_during_the_wait_says_nothing`, 929); the application's exit cancels the listener's waiter (`the_dialogs_application_exiting_ends_the_wait_and_cancels_it`, 946).
- A: the destruction, handled at once, clears the postponement: nothing.
- B1: the outpost's new root-matched destroyed path answers not there: nothing. B0: the loop finds the window gone: nothing.
- NVDA: no capture; its gainFocus on a dead object reads nothing or fails.

### The dialog shown later than any bound

- Today: read with its text whenever it is shown, up to 13 s, keys held meanwhile.
- A: read with its text whenever it is shown, keys never held.
- B1 and B0: `DIALOG_SPOKEN_HIDDEN` at the bound, nothing at the later show. Import's 221 ms is close to a 300 ms bound; on a slower machine or GitHub's runner the scenario's speech would change.
- NVDA: without its text if its pass ran before the show.

### The application hanging while hidden

- Today: `"cancel Listener Hung"`, "not responding" (`a_dialog_hung_during_the_wait_is_not_responding`, 975); the window joins the recovery set.
- A: no request is out, so no hang check and no sound until something reads the window (a key, the show's re-take).
- B1: the parked read is a request out: the live check stops it as hung, with the sound; the table answers it cancelled. B0: the same in Core; the worker's loop goes on to the bound and its read sticks in the hung window, a late answer dropped.
- NVDA: no sound (Verbatim-only).

### The outpost or the listener ending meanwhile

- Today: listener `DIALOG_SPOKEN` after the replacement's show (`the_listener_ending_during_the_wait_...`, 995); outpost `DIALOG_SPOKEN` from the next generation then a silent re-take (`the_outpost_ending_during_the_wait_...`, 1025).
- A: listener, a re-take when the new listener is ready, so a show missed meanwhile is not lost: `DIALOG_SPOKEN`. Outpost: its re-take postpones again; the show's re-take speaks `DIALOG_SPOKEN`.
- B1: listener, nothing to do with the wait; the listener's re-take after it is a silent duplicate. Outpost: the parked read ends with the generation; the re-take parks in the next one: `DIALOG_SPOKEN`.
- B0: the same as B1.
- NVDA: no counterpart.

### A UI Automation focus in a hidden dialog

- Today: the completion made again: `SPOKEN_FOLLOWED` (`a_uia_focus_in_a_hidden_dialog_is_completed_again_once_shown`, 1084).
- A: the show's re-take reads the current focus rather than completing the same element: expected the same object, unproved.
- B1 and B0: the completion parks, then the same.

## 3. Each option's size, state, principles, costs and decisions

Counts: today's from `git diff --numstat bc99bc40^ 870bda79 -- crates`, test modules split; the rebuilds' estimated per file against the code that exists, counting product lines added or kept for this behaviour, not lines deleted.

### Today

- Product about 1,560 to 1,590 lines in seven crates, plus 394 in mockapp. Tests about 2,190 lines plus the 425-line fixture.
- State: Core's `AwaitShown` request and its table entry; the listener's waiter, token and record of requests out on two threads; `ended_wait` on two entry kinds, carried through merges and the limiter's cap; `stopped_at_once` in the checkpoint; the after-wait re-take; `answer_cancelling`. Messages: `HiddenDialog`, `AwaitShown`, `Shown`, `LeftHiddenDialog`. Timer: the 13 s due time.
- Costs: one listener round trip and two Core wakes per hidden dialog; no tell per show; keys held to the show unless a key or an ending.

### A: postponement

- Product about 370 lines (A's per-file figures checked against the code; with the fence correction and a foreground-only drop about 380 to 400): model about 35 plus about 20 for a new event kind; ia2 about 32; listener about 2 to 10; outpost about 75; Core about 230 (shown.rs about 90, msaa_run about 60, uia_focus, retake, entries, menus, keys, state); event queue changes reverted. Tests about 300 (A's list) plus about 40 for the fence and the cross-process order of the show tell and the hidden answer: about 340; re-recorded fixture.
- State added: one Core fact, the postponed dialog, `None` or `Some(window)`; one event kind. Removed: everything of today's above but `HiddenDialog`. No request, waiter, timer or lifecycle in two places.
- Principles against today: better on one owner per fact, invalid states (no `ended_wait`), no hand-written concurrency, lifecycle count, one answer path (no waiter), owner records; decisions seen in order better (keys, shows and answers in one order). Worse on wake-ups if every show is told. Not addressed by A: events on the old focus (a name or state change of the owner's button) that arrive while postponed are judged against it before the dialog, where today they wait behind; NVDA would judge them after the dialog's gainFocus.
- Costs: per window-object show on the desktop, a tell (its facts, a pipe message, a Core wake), unmeasured; with the foreground-only drop, one `GetForegroundWindow` per show in the listener and a tell only for a window shown while in front. No added delay; keys never held.
- Changes to decisions of record: ADR 0003, "Readers tell, Core asks", "the listener's requests to be told when a window is shown (Dickson, 2026-11-19)" goes; with the drop, "A reader drops an event only by NVDA's local checks that cannot hang ... and by the listener's drops on an event's own cached class and automation id" gains a drop that is neither. "Who holds what", "The listener has one kind of request waiting on an event, which Core can cancel: 'tell me when this window is shown', kept on its WinEvent thread from the request to its one answer (Dickson, 2026-11-19)" goes, replaced by a tell. "Limits accepted", "so does a hidden dialog that is never shown, with no key pressed and no foreground, menu or focus elsewhere (Dickson, 2026-11-19)" goes, and "for a hidden dialog, the wait for its show reads the focus once, at the show" becomes the re-take at its show. "What Verbatim keeps beyond NVDA" gains the show tell and the postponement. Focus design: "Core asks the listener to tell it when the window is shown ..."; "The wait ends early when the user is elsewhere ..."; "A key pressed while the dialog is hidden, any key whose press cuts speech, a modifier alone included, ends the wait too ..."; "A dialog never shown, with no key and no other ending, holds the queue until the due time, 13 seconds, then ends unspoken." The parity ledger's ruled list under "A dialog's text".

### B1: parked read

- Product B says about 250; my estimate 280 to 360: model about 27; ia2 12; outpost asked.rs about 70; outpost-logic table kind 100 to 150 (B: 75); outpost worker 40 to 70 (B: 35), since `answer` (`asked.rs` 62) answers in one call today and must hand a parked ask back; Core about 30, with `read_shown`, `Hidden<T>` and the after-wait re-take removed. Tests about 330 (B's list); re-recorded fixture.
- State added: one parked kind in the outpost's waiting table (an ordinary request out, its answer held in the table); a poll timer in the outpost; a bound constant. Removed: the listener's waiter and record, Core's endings, carry, checkpoint record and re-take, and the replies `HiddenDialog`, `Shown`, `LeftHiddenDialog`.
- Principles against today: better on one owner per fact (visibility read where the read is), invalid states, concurrency, lifecycle count, owner records; equal on one answer path. Worse on "a loop never times a wake it will not act on" only if the poll outlives the request (it does not: cancel answers it). A bound that changes speech by machine speed is a timing of Verbatim's own on the speech path.
- Costs: up to about 30 local checks a dialog on the outpost's timer (bound 300 ms over 10 ms); no Core traffic; keys and later entries held up to the bound, the box and the closed dialog up to one or two bounds late, caret keys past 150 ms silent.
- Changes to decisions of record: the same ADR sentences as A on the listener's request and "Readers tell"; the never-shown limit replaced by the bound; "Core holds every rule", the outpost's list of mechanical work ("read, compare a caret with a starting point, ... move the caret") gains "wait for a window to be visible within a bound Core names"; focus design as A, plus "A dialog's name and text are read only once it is shown (Dickson, 2026-11-19)" qualified by the bound; the ruled list under "A dialog's text": "the user elsewhere before the show ... says nothing for the hidden dialog" is broken for a tray menu and a focus that executes, and "a key pressed before the show reads the dialog at once, without its text" no longer holds.

### B0: inline wait

- Product about 120 to 170: B1 without the table kind and the worker's hand-back, plus a loop of about 30 lines inside the read. Tests about 250.
- State added: none in any table; the worker is busy in a loop. Removed: as B1.
- Principles: as B1, plus worse on the worker: requests from outside the queue for that application (a terminal's output reads, the taskbar's, an extension's) and the outpost's own handling of its tells wait behind the loop, and every foreground cancel abandons and replaces the worker thread.
- Costs: as B1, plus a thread replaced per foreground cancel.
- Decisions of record: as B1, and "An outpost has one worker making every call into its application, one at a time, abandoned and replaced only when Core cancels a request it is still inside because the foreground moved away" now covers a worker not inside any call.

### The fact that has changed since the decisions

For every option, the only new facts since 2026-11-19 are the built size (about 1,560 product lines and 2,190 test lines, against the complexity target of about 1.3 times NVDA's code and NVDA's zero lines for a wait) and the fix commits after the review. The recovery set (A's precedent) and the not-in-front row (B's) existed when the wait was ruled; they are arguments, not changed facts. Neither study states how its option fits the planned milestones beyond B's line on M7 and M5; A is silent on them.

## 4. A fifth option

I did not find one that beats all four. Two were weighed:

- A corrected: A's postponement with the key's read stamped with the postponed entry's older fence, and the listener's show tell limited to the window in front as the default. It keeps today's speech everywhere except the modifier alone, the background order and the hang sound, and costs about 400 lines. It is A with its error fixed, not a new shape.
- A's postponement with the show learned from the outpost rather than the listener: Core's facts ask the outpost, through the existing facts-owned request path (`Logic::wanted` and `facts_answered`, `crates/verbatim-core/src/event_queue/mod.rs` 285 to 303, used by `terminal/reading.rs` 173 to 184), to answer when the window is visible, parked in its table; the answer joins the re-take. No listener change and no tell per show, but a request held in Core's table and the outpost's at once, a poll that runs until Core cancels it or the request's 11-second due time (about 1,100 checks for a dialog never shown), and a cancel to send when the postponement clears. Better than A on tells, worse on state and wake-ups; not better on every axis.

## 5. Where each study is wrong or silent, and what to measure first

Study A:

- Wrong: the key's speech under the rebuild is not today's as written (fence); today's speech after a modifier alone is silence, not the hidden dialog.
- Silent: events on the old focus judged before the dialog while postponed; the fence a show re-take takes; the cost of the show tells (it names the measurement but gives no figure); the milestones it touches; a caret key's timing.
- Measure first: the rate of window-object shows (`EVENT_OBJECT_SHOW` on `OBJID_WINDOW`, `CHILDID_SELF`) across the desktop, and how many name the foreground window, in a recording of ordinary use (typing, menus, File Explorer, a browser) with `verbatim-event-record`; the listener's processor time per told show.

Study B:

- Wrong: the not-in-front row covers only foreground candidates; the table kind is about twice the size it says; "audible speech no" for a key holds for typed characters but not for caret keys held past 150 ms.
- Silent: the caret key's release; the expired-focus drop in the tray case; speech for a dialog slower than the bound on a slower machine or runner, which makes `quit_inside_a_file_dialog` depend on timing; that two of its differences break Dickson's ruled option C.
- Measure first: the foreground-to-show delay over more dialogs and machines (cold starts, Office, an Open dialog in a slow application, GitHub's runner), to see how often a 300 ms bound is passed; the time from a caret key's press to its turn while a dialog is hidden, today and with a parked read.

For both, before a build:

- NVDA captures, since spoken expectations come only from captures: a typed character, a caret key and a modifier alone pressed while a dialog is hidden; a tray menu over a hidden dialog-based main window; a message box raised while its owner dialog is hidden; Tab pressed while hidden; a dialog never shown. Recordings with `verbatim-event-record` of the real applications behind the stand-in's `owned-box` and tray variants, which the coverage row says still wait for theirs.
- A re-recorded Import fixture under the new protocol, and the replay test's substring match replaced by an exact place.
- A reducer test, at no cost, settling whether speech made inside a key's entry is fenced by that key's cut.

## My reading

The facts point to the postponement, corrected. It removes the most state (one Core fact for a request, a waiter on two threads, a carry and a checkpoint record), holds no key and sets no bound, and keeps today's words in every sequence a test states, once the key's read is stamped with the older fence; its new speech (the dialog after a modifier alone, a background entry before the dialog) follows NVDA's arrival order. Its cost, a tell per window show, is unmeasured, but its fallback of telling only the foreground window's show is cheap and cannot lose a show of a dialog hidden at its foreground. The parked read is smaller in Core and simpler to reason about, but it trades the endings for a bound that holds keys, silences caret keys past 150 ms, breaks the ruled option C in two sequences and makes Import's speech depend on the machine's speed against a figure close to Import's own. The inline wait is smallest but blocks the application's worker. This is my reading, not a choice; the show count and the captures above should come before either build.
