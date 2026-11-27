# Core's focus rules: merged and verified record (2026-11-20, at ba89dca7)

Area: `crates/verbatim-core/src/focus/` (all), `src/focus_rules.rs`, `src/focus_speech_tests.rs`, `tests/replay_validity.rs`, `tests/hung_menu_end.rs`, `tests/fixtures/`. Paths are relative to `crates/verbatim-core` unless another crate is named. Every quotation below was compared with the file today; line numbers are today's.

## 1. Size

Recounted with `wc -l` and a filter that drops blank and comment lines ("code"). Product is each file up to its inline `#[cfg(test)]` block.

| Part | Files | Lines | Code |
|---|---|---|---|
| One definition, duplicate tests, acceptance, backend verdict | validity.rs 1,055, duplicate.rs 71, acceptance.rs 138, backend.rs 316 | 1,580 | 1,122 |
| MSAA run, menus, UIA focus, narrow rule | msaa_run.rs 451, menus.rs 303, uia_focus.rs 312, narrow.rs 47 | 1,113 | 839 |
| Execution, the two versions, focus object, re-take, follow, mod | execute.rs 707, versions.rs 424, object.rs 67, retake.rs 318, follow.rs 234, mod.rs 56 | 1,806 | 1,225 |
| Plain reader (ruled 2026-11-19) | plain.rs 522 | 522 | 343 |
| Dialog read once shown (ruled 2026-11-19) | shown.rs 391 | 391 | 254 |
| Shared rules | focus_rules.rs 331 | 331 | 170 |
| **Total product** | | **5,743** | **3,953** |

Tests: 877 inline lines (acceptance 245, backend 283, duplicate 180, focus_rules 169) and 9,948 in files (focus/*_tests.rs 5,718, focus_speech_tests.rs 2,705, replay_validity.rs 1,210, hung_menu_end.rs 315): 10,825 lines, 322 tests (both reviewers' count is right), plus 727 fixture lines. 1.9 test lines per product line.

NVDA's counterpart, whole functions in the pinned source, counted the same way:

| NVDA | Lines | Code |
|---|---|---|
| `IAccessibleHandler/__init__.py` 521-617 `winEventToNVDAEvent` | 97 | 79 |
| `IAccessibleHandler/__init__.py` 692-1008 focus, NVDA focus, desktop switch, foreground, show, destroy, menu start, fake focus | 317 | 257 |
| `IAccessibleHandler/__init__.py` 1019-1105 `pumpAll` | 87 | 68 |
| `eventHandler.py` 48-171 queue and pending, 312-361 `executeEvent`, 378-431 `doPreGainFocus`, 481-599 `shouldAcceptEvent` | 347 | 278 |
| `api.py` 59-205 `setForegroundObject`, `setFocusObject` | 147 | 117 |
| `UIAHandler/__init__.py` 96-140 class lists, 874-959 focus handler, 1206-1382 window verdict, 1516-1580 native element | 373 | 271 |
| `NVDAObjects/window/__init__.py` 338-362, 444-490 class normalisation; `NVDAObjects/IAccessible/__init__.py` 863-893 duplicate and focused-state tests | 103 | 98 |
| **Total** | **1,471** | **1,168** |

Ratio: 3.9 by lines; 3.4 by code, and 3.9 by code once the roughly 170 NVDA code lines that are reader-side in Verbatim (the probe, `getNearestWindowHandle`, `requestEvents`) are left out. Both reviewers' 3.9 by code stands on that basis; B's NVDA total (about 1,325) left out `executeEvent` and the destroy and show handlers, A's (about 1,540) included the whole of `eventHandler.py` 48-599. Without the two ruled features NVDA has no code for (plain.rs and shown.rs, 597 code lines), the ratio is 2.8 by code. NVDA has no unit tests of this code.

What the extra lines buy (merged; both reviewers agree):
- The cross-process hand-off forced by D9 and D13: request types and their decode, the ancestors Core knows, the UIA two-request chain, the re-take after a reader ends, `Follow`. About 1,300 lines.
- Behaviours NVDA lacks, each ruled: the plain reader (ADR 0003 line 100, Dickson 2026-11-19), the dialog read once shown (focus.md "A dialog read once it is shown", Dickson 2026-11-19), the identity check (ADR 0004, 2026-11-18). About 1,000 lines.
- Nothing: the duplication and dead code below, about 620 lines removable now plus about 150 through the two defect fixes.

Deletion target:

| Part | Product lines | Test lines |
|---|---|---|
| Remove now (F3 to F11, F13) | about 620 | about 270 removable; the desk harness merge (up to about 600) needs its feasibility shown first |
| Through the defect fixes (F1, F2) | about 150 to 200 | about 180 net (230 fence tests become one table of about 50; about 70 added for F1 and F2) |
| If Dickson rules for the removal (Q2, the carry-over) | about 110 | about 135 |
| Planned phase (F14, M6.4) | about 3 | 0 |

Q1 (the plain reader) is a cost question with no removal proposed, so it carries no figure here.

## 2. Findings

| id | title | class | found by | file and line | what goes | product saved | test saved | speech changes |
|---|---|---|---|---|---|---|---|---|
| F1 | The committed focus held in two records joined by different rules; a re-take makes them disagree and the next focus is silenced | Defect | both (sequence B's, verified) | focus/execute.rs 178-230, 481-495; reduce.rs 1842-1910 | one record of the committed line; `chain`'s second join and its three agreement checks | about 100-150 | adds about 40 | yes: the silenced focus is spoken |
| F2 | "A failed try ends the fence" written at nine sites with three rules | Defect (ReaderEnded on the UIA listener read and completion); Question Q3 (a reader's refusal on the UIA path) | both | msaa_run.rs 327-347; menus.rs 134-246; retake.rs 288-297; uia_focus.rs 142-148, 194-210, 274-303 | one `tried(outcome)` and one per `Reason` | about 45 | about 180 net | yes: the defect's sequence speaks the item |
| F3 | focus_rules.rs mostly dead behind a stale module doc | Remove now | both | focus_rules.rs 1-14, 24-116, 163-243, 253-264, 316-330; lib.rs 98 | the dead items, the re-export, the doc | about 185 | about 130 (4 tests) | no |
| F4 | A second copy of NVDA's class lists in Core, already drifted from NVDA | Remove now | both (drift B only) | focus/backend.rs 33-171; verbatim-reader-rules/src/class.rs 17-56, 57-102, 266-319 | Core's lists and `normalize` | about 140 | about 96 (8 tests) | no |
| F5 | Read depth written as types, two traits over the same markers | Remove now | both | msaa_run.rs 31-166, 256-309; retake.rs 57-87 | marker types, both traits, `TurnDepth`, the repeated arms | about 100 | 0 | no |
| F6 | Dead code behind a blanket `expect(dead_code)` with a stale reason | Remove now | both | focus/mod.rs 5-11 | the attribute and what it hides | about 50 | about 14 (1 test) | no |
| F7 | `ObjectClass` has one variant and is matched as if it had more | Remove now | both | verbatim-model/src/request.rs 738-741; duplicate.rs 32, 60 | the enum and its 35 references | about 30 | small | no |
| F8 | "The run before executed a focus" passed as a flag in shared state | Remove now | both | state.rs 591-593; entries.rs 501; msaa_run.rs 356, 379, 394, 416, 447 | the field and the take | about 10 | 0 | no |
| F9 | `judge` and `judge_read` are one match written twice | Remove now | both | validity.rs 316-346 | one of the two | about 25 | 0 | no |
| F10 | The MSAA execute tail written twice | Remove now | B only, confirmed | msaa_run.rs 430-451; menus.rs 167-181 | one function | about 20 | 0 | no |
| F11 | Smaller copies: `Why` and `HungRead`; the non-result arms of `Outcome` written three times; the foreground relation recomputed; old-dump `serde(default)` | Remove now | both (relation and serde A only, confirmed) | retake.rs 33-42, 273-278; shown.rs 321-330, 350-358; event_queue/driver.rs 861-870; execute.rs 148-150; versions.rs 162, 170 | the copies | about 60 | 0 | no |
| F12 | The menu check compares a cloned `Committed` by value, where NVDA compares identity | Defect | A only, confirmed | menus.rs 100-104, 188 | the clone; compare the committed node | a few | 0 | yes, in one corner: needs a capture |
| F13 | The crate guide describes removed mechanisms and dead items | Remove now (documents) | both | books/dev/src/reference/crates/verbatim-core.md 26-72, 485-735 | the stale passages | 0 code | 0 | no |
| F14 | `observed_at_us`, 0 for Core's own reads, passed to a callee that ignores it | Planned phase: M6.4 terminal reading rebuild | A only, confirmed | execute.rs 611; verbatim-text/src/terminal_reading.rs 669-670 | the argument | about 3 | 0 | no |

### F1. The committed focus in two records (both; Defect)

The two records, both of the committed focus: `FocusState { object: Option<FocusContext> }` (focus/object.rs 18-22), holding the snapshot and `ancestors: Arc<[NodeSnapshot]>`; and `Committed::Read(Box<Kept>)` (focus/versions.rs 222-240), `Kept` holding `node`, `role`, `own_window` and `ancestors: Arc<[KeptAncestor]>` (versions.rs 131-172). One execution writes both: execute.rs 513 `said.extend(crate::reduce::execute_report(state, &ready.report, left));` then 601 `state.versions.execute(ready.kept, origin.fence);`; the desktop switch does the same at input_desktop.rs 99 `state.versions.commit_own(true);` and 102 `state.focus.sleep();`. Fields are copied across: execute.rs 421 `role: node.role,` and 422 `own_window: own_window(&read.focus.read.values),`. The module doc says why: "The announcement code and the reducer's focus context are shared with the old focus path until the old types are deleted" (execute.rs 5-7).

The two joins differ. `FocusContext`'s line comes from `focus_ancestry` (reduce.rs 1842-1910), which meets the old line by `lowest_common_ancestor`, matching "by node id, or by window handle when both are windows", and keeps the old line's entries to the meeting point: "The previous line's entries are kept to that point, as NVDA keeps its old objects, so a window matched by its handle keeps the name it was entered with" (reduce.rs 1823-1825). `Kept`'s line comes from `chain` (execute.rs 178-230): the answer's ancestors, then the committed ancestors above the place the walk met. The three agreement checks:

    if itself.id != committed.node {
        return None;
    }

(execute.rs 210-212);

    for above in committed.ancestors.get(above_from..)? {
        let node = context
            .ancestors
            .iter()
            .find(|snapshot| snapshot.id == above.node)?;

(execute.rs 220-224); and validity.rs 394 `let snapshot = committed.filter(|snapshot| snapshot.id == kept.node)?;`, which only drops the identity check. A `None` from `chain` makes `ready` return `None` (execute.rs 338-339) and `execute` return `Executed::default()` (493-495): nothing is said and nothing is logged, under the doc's "a reader's fault" (481-482), although Core wrote both records.

The sequence, derived step by step from the code:

1. In Notepad's Find dialog, "Find what" has the focus, read through MSAA. Both lines hold the dialog's client object, d1. It is a window entry: the outpost gives a client object at child id 0 its own window (verbatim-ia2/src/acquire.rs 2240-2243), carried as `NodeDetails.window`, which `ObjectKey::is_same_object` matches by handle (verbatim-model/src/tree.rs 700-703).
2. Notepad's outpost generation ends. `reader_ended` marks the committed focus ended and appends a re-take (retake.rs 234-251). The re-take reads with no known ancestors (retake.rs 261 `known: Vec::new()`, and `retake` uses `Known::default()` at execute.rs 668), so the answer's `met` is `None` and its ancestors carry the new generation's ids: the dialog is d2.
3. The re-take reads the same, so `take_silently` runs (execute.rs 672-677): `focus_ancestry` matches the new line to the old one by window handle (the edit's own window, or the dialog's), and keeps the old line's entries, d1. `commit` then stores `Kept` with the answer's ancestors, d2 (execute.rs 693, 601). If the re-take reads differently, `execute` runs instead and the same join keeps d1 the same way.
4. Tab moves to "Find Next". Its MSAA focus is read with the known ancestors of the new generation: `known_with_places` sends `KnownAncestor::Node(d2)` and `KnownAncestor::Window(dialog)` at place 1 (versions.rs 392-409). The walk meets the dialog; `chain` looks d2 up in `context.ancestors`, which holds d1; `find` misses, `chain` returns `None`, and "Find Next" is not spoken.

So the mismatch is reachable after every re-take whose focus or ancestor is a window object, a common case in Win32 dialogs. No test covers a focus after a re-take that meets an ancestor (retake_tests.rs has none). focus_speech_tests.rs 2244-2299 shows the same divergence at the desk level ("The line keeps the frame host's entry for the frame"), but in production that case meets the window as a known ancestor and stays consistent; the re-take is the path that breaks.

Fix: one record of the committed focus's line, joined once, with what `Kept` adds per entry beside each snapshot; `chain`'s lookups, the agreement checks, the copied `role` and `own_window`, and `FocusState`'s second copy go (about 100-150 product lines; reduce.rs's readers of `state.focus` change too, the reducer pair's area). Speech: the silenced focus is spoken as any Tab; its exact words need a capture. Cheapest test to add: a re-take that keeps an old window entry, then a Tab inside it (about 40 lines in retake_tests.rs).

### F2. The "tried" rule at nine sites (both; Defect and Q3)

The ruling. ADR 0004, "Dickson's decisions" (dated 2026-11-18), line 75: "A failed try ends the identity check's fence, so the mouse return to a list after a click whose read failed is spoken." `books/dev/src/explanation/design/focus-validity.md` 73: "A candidate tried at its turn and found refused, not there, or whose reader ended, clears the record. A candidate refused before any read, or skipped as read through UI Automation, leaves it." ADR 0004's limit (line 86): "A stale candidate refused between a split pair clears the fence, so the item is said twice."

The sites, each with the outcome it handles and what it does:

1. msaa_run.rs 327-339, `answer_of`, the outcome of a run candidate's read at its turn (`read_in_full`) and of a menu start's read (`read_full`):
   `Outcome::NotThere | Outcome::ReaderEnded => true,`
   `Outcome::Refused(_) | Outcome::NotResponding | Outcome::NoLongerAwaited(_) => false,`
2. msaa_run.rs 343-347, `refused`, `if reason.tried() { cx.facts_mut().versions.tried_and_refused(); }`, for the walk's refusals (369-371), the identity answer's refusal (396), an unread answer (404) and the judgement of the read (412). `Reason::tried` is `matches!(self, Self::NotThere | Self::NotFocused | Self::NotMenuPopup)` (validity.rs 112-114).
3. menus.rs 134-137, 149-152, 159-162 (menu start: the judgement before the read, an unread answer, the judgement of the read) and 231-235, 242-246 (menu check): the same `if reason.tried()` / `if unread.reason().tried()`.
4. menus.rs 219-228, the menu check's outcome: `if matches!(outcome, Outcome::NotThere | Outcome::ReaderEnded) { facts.versions.tried_and_refused(); }`
5. retake.rs 288-297, the current-focus read's outcome (startup, re-take, recovery, return): `if matches!(outcome, Outcome::NotThere | Outcome::ReaderEnded) {`
6. uia_focus.rs 142-148, the listener's read of a UI Automation focus: `refused_or_not_there(cx, &outcome);`, whose body (295-303) is `Outcome::Refused(crate::event_queue::Refused::Reader(_)) | Outcome::NotThere`.
7. uia_focus.rs 194-210, the completion: `Outcome::ReaderEnded => { cx.facts_mut().focus_read_failed(); return false; }` (198-201) leaves the fence; the other failures go through `refused_or_not_there` (206).
8. uia_focus.rs 274-280, the read afresh: `refused_or_not_there(cx, &outcome); if matches!(outcome, Outcome::ReaderEnded) { cx.facts_mut().versions.tried_and_refused(); }`.
9. uia_focus.rs 110-121, the UI Automation focus refused on its tell's facts: no clearing; none of its refusals (NoWindow, Verdict, Override, HiddenFrame, Acceptance, KeyboardFocusLost) is a `tried` reason, so this agrees with "refused before any read ... leaves it".

The disagreements:

- `ReaderEnded`: clears at sites 1, 4, 5 and 8; leaves at sites 6 and 7. The ruling names "whose reader ended", so sites 6 and 7 are **defects against the ruling**. Core's `ReaderEnded` is also what an outpost or listener answers when a read fails while the reader lives: the wire's `Ended` (event_queue/driver.rs 883), which the outpost sends for `ReadError::Failed` (verbatim-outpost/src/outpost/asked.rs 270-277) and the listener for a cancelled or abandoned read (verbatim-listener/src/listener/asked.rs 304-314). So the gap is reached without any reader ending.
- `Refused(Refused::Reader(_))`: clears at sites 6, 7 and 8; leaves at sites 1, 4 and 5. On the MSAA paths it is unreachable from an outpost today: the outpost answers an `MsaaRun` only with a result (asked.rs 1180-1201) and a current-focus read with a result or "not there" (asked.rs 1358-1383). On the UI Automation path the refusals are three: `NotFocused` from the listener (listener/asked.rs 745), the counterpart of `Reason::NotFocused`, so clearing agrees with "found refused"; `ReadThroughMsaa` from the outpost's verdict (outpost/asked.rs 160), the counterpart of the MSAA skip that the ruling says "leaves it", which the ruling does not name for UI Automation: **unruled** (Q3); and `Unsupported`, a reader's fault logged as an error: **unruled**.
- `Refused::Undecodable`, `NotResponding` and `NoLongerAwaited` leave the fence at every site: consistent, and unruled; identity_tests.rs 362-386 pins `NotResponding` as keeping it.
- A hung window answered with what is in front (`InFront`, sites 4, 5 and 8 hand it to `plain::decide`) clears nothing: consistent, unruled.

The defect's sequence: a click speaks an MSAA list item; it becomes the readers' last focus with fence F. With no key, a click into an application read through UI Automation raises its focus, fence F; the listener's read, or the completion, answers `Ended`. The fence stays F (sites 6, 7). A click back on the list raises the list's focus on the same item, fence F; `validity::named` applies the identity check (validity.rs 380 `if last.since != Some(candidate.fence)` does not return), the outpost answers "the same" (`Reply::CandidateSame`), `judge_same` makes it a duplicate, and nothing is said. By the ruling the fence was cleared and the item is spoken, as the mouse-return replay says "Alpha" "1 of 3" for mockapp's list (tests/replay_validity.rs 974-987; NVDA's speech for this exact sequence needs a capture). identity_tests.rs 342-359 tests `WireOutcome::Ended` only on the MSAA run, and 426-449 only `NotThere` at the completion, so no test catches the gap.

Fix: one `tried` on the outcome and one on the `Reason`, applied in one place per answer; about 45 product lines go, and the per-path fence tests (identity_tests.rs 342-386, 426-449, 654-700; menus_tests.rs 495-541; retake_tests.rs 372-404; validity_tests.rs 665-681, about 230 lines) become one table of the outcomes (about 50), which then covers the completion's `Ended`.

### F3. focus_rules.rs mostly dead (both; Remove now)

`pub mod focus_rules;` (lib.rs 67) hides unused items from the compiler. `grep -rnw <name> crates xtask uia-facts --include=*.rs`, excluding focus_rules.rs, finds no use of `ForegroundRelation` (0), `HeldNode` (0), `RepeatEvidence` (0), `repeats_focus` (0), `change_candidates` (0), `SelectionTarget` (0), `selection_target` (0), `already_entered_ancestors` (0); `is_selection_container` only at lib.rs 98 `pub use focus_rules::is_selection_container;`. Live: `change_target`, `ChangeKind`, `ChangeTarget`, `is_sensitive_slider`, `lowest_common_ancestor`, `same_object` and the `EnteredKey` trait (its `HeldNode` impl, 253-264, goes). The module doc (1-14) says "the focus listener and the outposts apply the same definitions where a judgment must come before an element is read", which ADR 0004 line 17 ended ("The readers report facts and judge nothing"). About 185 product lines. Tests: of the five, `a_repeat_needs_every_piece_of_evidence` (337-353), `a_selection_is_spoken_on_the_focus_its_ancestors_or_inside_a_focused_list` (389-423), the `held` helper (424-438), `a_container_is_entered_unless_the_lines_meet_at_or_below_it` (439-457) and `two_windows_are_one_container_exactly_when_they_are_one_windows_object` (458-499, whose rule verbatim-model tests in speech.rs 556-590) go, and two `change_candidates` asserts in the fifth: about 130 lines. This is the clearing-up step of "Readers tell, Core asks" (docs/roadmap.md 147-149: "the messages and code nothing uses are deleted"), so doing it now is that step's own work.

### F4. Class lists copied into Core (both; drift B only; Remove now)

focus/backend.rs 33-171 holds `CLASS_MAP` (45 pairs by count, not 46), `GOOD`, `BAD`, `SHELL_ROOTS`, `inner_name`, `normalize`, `by_class`; verbatim-reader-rules/src/class.rs holds the same (`CLASS_MAP` 57-102, `GOOD_UIA_CLASSES` 266, `SHELL_ROOT_CLASSES` 284, `BAD_UIA_CLASSES` 299), and its doc says the lists are shared "so the two always agree" (class.rs 5-7). Compared as sets today: the map's 45 pairs, the good, bad and shell lists all agree. The parsing has drifted: Core's `inner_name` requires digits (`if digits == 0 { return None; }`, backend.rs 128-130) and takes the first `.app.` (`rest.find(".app.")`, 132); reader-rules takes zero or more digits (`trim_start_matches`) and the last (`rfind(".app.")`, class.rs 41-45); NVDA's pattern is `re.compile(r"^WindowsForms[0-9]*\.(.*)\.app\..*$")` (`nvda/source/NVDAObjects/window/__init__.py` 25), so reader-rules matches NVDA and Core does not. Core cannot depend on reader-rules (tier rule). Either shape works: A's, the lists and `normalize` in verbatim-model, used by both; or B's, the listener's tell carrying the class verdict for `JudgedWindow.by_class`, which changes a wire message. About 140 product lines and backend.rs's 7 list tests (343-424) plus the dead `Judged` variants' test (443-456), about 96 lines. Speech: no in practice; only a Windows Forms class with no digits or two `.app.` parts changes, to NVDA's verdict. Out of area, for the outpost pair: `Arbitrator::new(extra_good)` is called with `&[]` at its one product site (verbatim-outpost/src/outpost/mod.rs 848).

### F5. Depth as types (both; Remove now)

msaa_run.rs 31-166 defines `trait ReadDepth` and the markers `FactsOnly`, `FullIfFocused`, `Full`, `MenuFull`, `UnlessSame`, with `UnlessSameAnswer` (74-82), `TurnAnswer` (84-92) and `RunRequest<D>` (`depth: PhantomData<D>`, 177); `TurnDepth` (256-265); `read_in_full` (271-309) repeats one arm per depth. retake.rs 57-87 adds `trait FocusReadDepth` over the same markers. The wire already has `Depth` and `FocusDepth`, and `Answered::parse(answer: CandidateAnswer, depth: &Depth)` (validity.rs 208) parses by value. Neither reviewer named why the markers exist: `Request::decode` is an associated function with no access to the request (`fn decode(reply: Reply) -> Option<Self::Reply>`, msaa_run.rs 195), so the fix either gives `decode` the request or makes one parsed type for every depth. About 100 product lines. Speech unchanged.

### F6. Dead code behind `expect(dead_code)` (both; Remove now)

focus/mod.rs 5-11: `expect(dead_code, reason = "built beside the reducer; the focus entries switch over to it in a later step of build 1")`. Behind it, each checked by grep over crates/: `Versions::known_ancestors` (versions.rs 361-366), called only from versions_tests.rs (11 calls) and input_desktop_tests.rs 132; `Judged::MsaaCaret` and `Judged::UiaUnjudged` (backend.rs 224, 229), built only in backend.rs's test 447 and 452; `Retaken` (execute.rs 642-652), dropped by its one caller, retake.rs 313 `retake(&mut facts, &origin, &read, read_by).1`; `Executed::follows` (execute.rs 558), always empty since `commit` ends with `Vec::new()` (612), so `super::follow::send(cx, self.follows)` (571) sends nothing; the `Need` in `Validity::NeedsRead(Need)`, whose value only validity_tests.rs reads (product matches `NeedsRead(_)`). About 50 lines and 1 test; what more the compiler names once the attribute goes is not known without building.

### F7. `ObjectClass` (both; Remove now)

`pub enum ObjectClass { Plain, }` (verbatim-model/src/request.rs 738-741), matched at duplicate.rs 32 `ObjectClass::Plain => earlier == ids && *held_child <= 0,` and 60 `ObjectClass::Plain => false,` (both reviewers' line numbers were wrong). 35 references across crates. Its doc reserves it for Word, Outlook and Internet Explorer, which the roadmap places after the object-model bridge "planned around M8.5" (docs/roadmap.md 2060-2061), not a planned phase of the brief. About 30 lines.

### F8. `run_executed` (both; Remove now)

state.rs 591-593 `pub(crate) run_executed: bool,` set by the run (msaa_run.rs 379, 394, 447), read at 356, and taken by every entry: entries.rs 501 `let run_executed = std::mem::take(&mut cx.facts_mut().run_executed);`, passed to `menus::slot` (504). The fact is the run's result; returning it removes a field of the shared state. The change lands partly in the event queue's `Item::Menu` (another area). About 10 lines.

### F9. `judge` and `judge_read` (both; Remove now)

validity.rs 316-331 and 335-346 repeat the four-arm match, one with `facts.ok_or(())`, one with `Ok::<_, Infallible>(facts)`; `judge_same` (415-417) is `before_duplicate(candidate, view)`. About 25 lines.

### F10. The MSAA execute tail twice (B only, confirmed; Remove now)

msaa_run.rs 430-451 and menus.rs 167-181 both do `let Some(held) = read.msaa else { return; };`, build `ReadBy::Msaa { event: Some(held.event), held_child: held.held_child, }`, `Origin::of_tell(...)`, `execute`, `deliver`; retake.rs 158-170 builds the same `ReadBy`. About 20 lines.

### F11. Smaller copies (Remove now)

- `Why` (retake.rs 33-42) maps one to one onto `HungRead` at retake.rs 273-278 (both). About 15 lines.
- The non-result arms of `Outcome` are rewritten by hand at shown.rs 321-330 (`unanswered`) and 350-358 (in `read_shown`) (both), and a third time at event_queue/driver.rs 861-870 (`widen`), found while checking. `Outcome::map` (event_queue/request.rs 70-81) does not fit as is, since the result arms differ; one helper for the other five arms serves all three. About 20 lines.
- execute.rs 148-150 recomputes acceptance's relation (`window_to_foreground != Relation::Outside || root_owner_to_foreground != Relation::Outside || root_owner_is_foregrounds`), which acceptance.rs 124-131 decides (A only; A cited 362-369, which is test code). A few lines.
- versions.rs 162 and 170, `#[serde(default)]` on `Kept::selection` and `console_terminal`: old-dump defaults the project's rule forbids (A only). No committed fixture serializes `Kept` (no `"own_window"` in tests/fixtures), so nothing breaks. 2 lines.

### F12. The menu check compares a cloned `Committed` (A only, confirmed; Defect)

menus.rs 100 `let committed = cx.facts().versions.committed.clone();`, stored in the check, and `check` (188) returns when `cx.facts().versions.committed != check.committed`. `Kept` derives `PartialEq` over every field (versions.rs 130-131), `selection` included, which changes in place while the focus stays: caret_tell.rs 333-334 (`committed.selection.text_changed = false; committed.selection.baseline = Some(read.baseline);`) and changes.rs 348 via text_facts.rs 92-94 (`kept.selection.text_changed = true;`). NVDA compares identity: `if oldFocus is not api.getFocusObject(): return` (`IAccessibleHandler/__init__.py` 991-994). Sequence: an edit is committed; a popup menu opened from it closes with a popup end and a menu end and no focus event; within the 50 ms before the check, a caret tell's answer or a value change of the edit changes its selection facts; the check returns without reading. When the application's focus stayed on the edit, both are silent; when it moved with no event, NVDA's fake focus speaks the new focus and Verbatim says nothing (needs a capture). Fix: compare the committed node, and stop cloning. A few lines.

### F13. Stale crate guide (both; Remove now, documents)

books/dev/src/reference/crates/verbatim-core.md 26-72 documents the dead focus_rules items (`ForegroundRelation`, `RepeatEvidence`, `change_candidates`, `SelectionTarget`, `already_entered_ancestors`, `HeldNode`); 485-735 name `focus_queue` (2 mentions), `live_reread` (3), `plan_focus`, `needs_live_ancestry`, `first_cached_ancestor` (2), `Rereading`, `keep_duplicate`, `stale_floor`, `focus_stamps`; grep over crates/ finds none of these but `focus_queue`, which survives only in three stale doc comments in reduce.rs (207, 258, 413) naming a module that does not exist. Part of the clearing-up step (roadmap 147-149).

### F14. `observed_at_us` (A only, confirmed; Planned phase M6.4)

execute.rs 611 `crate::terminal::reading::committed(state, terminal, follows, origin.observed_at_us);`, where `observed_at_us` is 0 for Core's own reads (execute.rs 303, and 626 for the again path); the callee's `focus(at_ms)` does `let _ = at_ms;` (verbatim-text/src/terminal_reading.rs 669-670). M6.4 rewrites terminal reading.

Resolved (2026-11-27): `observed_at_us` is gone; `committed` takes the step's `Now`, and the rules' `focus()` takes no time.

## 3. Test findings (verbatim-core)

322 tests: focus_speech 88, shown 32, plain 31, validity 28, backend 20, identity 19, menus 17, acceptance 16, versions 15, replay_validity 14 (one ignored), duplicate 13, follow 10, retake 10, focus_rules 5, hung_menu_end 4. No generated test and no allowance in the area.

Removable:
- focus_rules.rs: 4 tests and the helper, about 130 lines (F3).
- backend.rs: 7 class-list tests (343-424) and the `Judged` variants' test (443-456): 8 tests, about 96 lines (F4, F6).
- Duplicate cases: duplicate.rs's 13 tests test its two pure functions; validity_tests.rs `a_uia_focus_on_the_last_focuss_element_still_focused_is_a_duplicate` (360-384) and `an_msaa_duplicate_and_the_repeated_item_are_judged_with_no_read` (450-473) repeat their cases through the rows. Keep the cheaper unit tests (the testing standard keeps the cheapest that can fail) and trim the two row tests to the case that places the row: about 30 lines. A's proposal to remove 11 of duplicate.rs's tests runs the other way and is not taken.
- versions_tests.rs's 11 calls of `known_ancestors` point at `known_with_places` once F6 lands.

Replaceable:
- The fence tests, about 230 lines in 7 tests, by one outcome table of about 50 (F2).
- replay_validity.rs: the nine sessions (`settings_list` 337 to `desktop_delete` 690, with `sessions()` 740-758) run only from `regenerate`, `#[ignore = "regenerates the committed fixtures; run explicitly"]` (1065-1067); the routine tests replay the committed fixtures and also assert Core's own recorded effect counts (`assert_eq!(counts, recorded, "the effect counts the session recorded");`, 794). So the sessions can drift from their fixtures unseen (both). Replace with B's shape: run each session in its test, assert its speech and compare its dump with the committed fixture, so `cargo xtask replay` keeps its inputs; A's shape (delete the nine fixtures, 117 lines) loses those inputs. About the same lines; no path left unrun.
- `the_recorded_settings_dialogs_destroys_reach_its_outpost_and_clear_cores_records` (887-949) asserts internals by JSON path (`state["queue"]["backend"]["probed"]`, `serialized["follows"]["named"]`, `serialized["plain"]["object"]`); assert the destroy's effects (B only, confirmed).
- focus_speech_tests.rs 1827-2150: 16 flat-review tests (B's count; A said 13), 324 lines, review commands in a focus file; `flat_review_moves_by_grapheme_cluster` (1923) repeats review.rs 167 `characters_are_grapheme_clusters`. Move them to the review pair. Both reviewers also say the file's announcement tests drive the whole desk to test reduce.rs's presentation; that is the reduce.rs pair's to count.
- The desk harness: src/desk_tests.rs (681 lines) and tests/desk/mod.rs (748) share 33 function names (A's count, verified; B said 25). One drives the crate-internal queue, the other the public reducer, so sharing one through `#[path]` needs the internal tests to drive the public reducer; the saving (up to about 600 lines) is not verified until that is shown. tests/desk is used by eight other test files, outside this area.

To add:
- A UI Automation completion and listener read answered `Ended` between a spoken list item and the return to it (F2): one row of the outcome table.
- A re-take keeping an old window entry, then a Tab inside it (F1): about 40 lines.

Noted, no change: `Committed::Own { source: Some(_) }` is written by no product path (the one product call is input_desktop.rs 99 `commit_own(true)`), so versions_tests.rs 180-200, identity_tests.rs 270 and validity_tests.rs 591 test a state nothing writes yet; ADR 0003 line 100 keeps it for later features ("an expression being explored in maths, an object a module makes, or a recognition result") (B only, confirmed).

Totals: about 270 lines removable now, about 180 net through F2's table, about 330 moved out of the area, and two tests to add.

## 4. Questions for Dickson

### Q1. The plain reader's cost (both; no removal proposed)

The ruling, ADR 0003 line 100: "A plain window, the ghost of a hung window found in front by a read NVDA makes itself, is committed as an ordinary object whose reader is Core's plain reader: a reader that holds no element, issues its own node ids, and answers every kind of request on its object at once from the facts the shell read, as NVDA's plain `Window` answers, so every command, today's and later ones, treats it through its own path with no case of its own (Dickson, 2026-11-19)." The speech, line 96: finding a ghost, "it commits NVDA's plain window object of the ghost as the focus, spoken, with no sound, and watches the hung window", heard "in 20 of 20 captures" (`NVDAObjects/window/__init__.py` 83-98, 162-181 and 299-306, checked: a hung window gets no API class, the focus window comes from the foreground window's thread, and its states come from its style). tests/hung_menu_end.rs 1-13 records NVDA's "hungwin (Not Responding)  window".

Its lines: plain.rs 522 (343 code); plain_tests.rs 1,141 (31 tests) and tests/hung_menu_end.rs 315 (4 tests), 1,456 test lines. Outside the area it also uses entries.rs 791-813 (the destroy and the answer hook) and the shell's live hang check (verbatim-app/src/hang_check.rs, 354 lines), which also serves the "not responding" answer to every request, so it is not the plain reader's alone. Commits: f23a42b1 (58 files, +2,933 -258; A said 41 files, B is right), d6ff64a0 (12 files, +251), 0c8804a5 (25 files, +531 -180), bc61c881 (52 files, +1,473 -279, shared with the destroy work).

What each part of plain.rs serves:
- Doc and imports, 1-50.
- `PlainKind` (52-79): the ghost, the destroyed ghost ("window invisible", Dickson 2026-11-19) and the desktop.
- The reader proper, answering every request kind (ADR 0003 line 100): `PlainObject` with its values, answers and relation (81-208, 90 code), `PlainReader` with its generations and the destroy (210-251, 29 code), `Asked`, `subject` and `answer` (253-327, 55 code). `subject` classes all 15 wire kinds, so each new request kind must be classed there.
- The decision table of ADR 0003 line 96 (`HungRead`, `decide`, `stood_for`, 329-428, 58 code): which reads commit the ghost or the desktop, and which play the sound and watch.
- The commits (430-522, 83 code): the ghost and the desktop committed through the ordinary `execute`.
- Tests: the decision table's rows (plain_tests.rs 169-423 and 1048-1141, about 350 lines), the reader's answers (424-641, about 220), the destroyed ghost (642-769, about 130), the commands on the plain window, report focus, navigator, review, say all, navigation, activate (770-901, about 130), its interactions with the listener's end, menu openings, duplicates, recovery and another application's ghost (902-1047, about 145), and helpers (1-168). hung_menu_end.rs feeds the shell's recorded order across the process boundary.

Question: is this cost, about 520 product and 1,460 test lines in the area, accepted as ruled, or should a cheaper shape be studied? No removal is proposed.

### Q2. The dialog-shown wait's carry-over (both)

The design of record, focus.md "A dialog read once it is shown" (Dickson, 2026-11-19): "A focus that ended a wait carries the dialog with it, through a merge or the limiter's cap too; should its run then execute nothing while the dialog is still in front, Core reads the current focus as a re-take, which waits for the show in turn." The carry-over: `mark_ending_focus` (shown.rs 228-257), `ended_wait_mut` (260-269), `merged` (271-282), `after_ended_wait` (373-390), the `ended_wait` fields, msaa_run.rs 353-358 and uia_focus.rs 98-100: about 110 product lines; tests shown_tests.rs 775-865 and 1206-1247, about 135. Sequence (shown_tests.rs 775 `an_ending_focus_refused_by_its_read_reads_the_dialog_again_once_shown`): Enter opens the hidden Import a Theme dialog; a stale focus outside it ends the wait and its read finds it unfocused. With the carry-over: the dialog is read again once shown, "Import a Theme" dialog with "File name:", then "File name:" edit (`DIALOG_SPOKEN`, shown_tests.rs 376-379). Without it: nothing until the next event in the dialog. NVDA's speech for this sequence needs a capture. Fact changed since the ruling: none beyond the cost; the question is only whether the corner is worth about 245 lines.

### Q3. A reader's refusal on the UI Automation path, in the "tried" rule (from F2; B raised it, the site analysis is the merge's)

Unruled: whether a UI Automation focus whose completion the outpost refuses as read through MSAA (`Refusal::ReadThroughMsaa`, outpost/asked.rs 160), or refuses as unsupported, clears the fence. Today it does (uia_focus.rs 295-303); the MSAA counterpart, a candidate skipped as read through UI Automation, leaves it by the ruling (focus-validity.md 73). Sequence: an MSAA list item is spoken; the item's list raises its own focus and the item's focus (a split pair); between them, a UI Automation focus elsewhere, whose window the outpost's probe judges MSAA, is refused `ReadThroughMsaa`. Clearing (today): the second focus of the pair is read in full and the item is said twice. Leaving: the identity check applies and the item is said once. The item's words in each case need a capture.

## 5. Cost of one behaviour

From `git show --stat`: f23a42b1, the ghost as NVDA's plain window, 58 files, +2,933 -258 (plain.rs 414, plain_tests.rs 895, event_queue driver.rs 208 and inbox.rs 109, verbatim-app hang_check.rs 227, request.rs in verbatim-model, six crate guides, focus.md, ADR 0003, test-coverage.md); 0c8804a5, the desktop focused when the check finds no window, 25 files, +531 -180; 9aba90be, the identity check, 55 files, +2,757 -1,134 (A said about 40), with msaa_run.rs 278, validity.rs 252, identity_tests.rs 845, validity_tests.rs 390.

A new kind of focus read touches: a wire request, reply and depth in verbatim-model; a Core request type with its decode and, today, a depth marker in two traits (F5); the outpost's handler; a row in validity.rs; its own copy of the fence rule (F2); `read_shown`'s wrapping; `execute`'s conversion into the reducer's report and the second record (F1); `plain.rs`'s `subject`, which classes every wire kind; a `HungRead` row and its `Why` (F11); tests through two or three harnesses; and five to seven documents.

Shared mechanisms that would cut it (both reviewers, merged):
- One focus-read request carrying a `Depth` value, with one parse and one place that turns any outcome into tried, failed or in front (F2, F5): a new candidate kind becomes a row and a depth, not a module.
- One record of the committed focus's line (F1): a behaviour that changes the line edits one join.
- One home for the class lists (F4): a class rule changes in one crate.
- One desk harness, if the internal tests can drive the public reducer.
- The crate guide pointing at focus-validity.md for the rows instead of restating them (F13).
- B's further idea, the plain reader answering through one generic "properties from facts" path so new request kinds skip `plain.rs`, belongs to Q1.

## 6. Dropped

- A's J (merge `LastFocus` into the committed record, the listener's token held twice): the duplication is real (retake.rs 219-223 clears the token in `versions.last` only, so `LastFocus::names`, versions.rs 122-125, fails and `caret_target` answers `None` until the next execution), but the proposal changes ADR 0003 line 100's decision ("Core keeps the committed focus and the readers' last focus as separate facts") with no changed fact named, and no caret tell lost in that window was shown.
- B's removal of two plain_tests.rs tests as duplicates of hung_menu_end.rs: not duplicates; plain_tests.rs 169 asserts the commit's records and the watch, hung_menu_end.rs asserts speech across the process boundary, and the cheaper one is the one B would remove.
- B's `opening_dropped` and `candidates_of` copies (menus.rs 288-300, validity.rs 594-606): an allocation with no measurement is not a cost under the standing rule, as B says itself.
- A's 11-test removal from duplicate.rs: reversed to a trim of the costlier row tests (section 3).
- Wrong line numbers corrected rather than dropped: `ObjectClass` matches (duplicate.rs 32, 60), acceptance's relation (124-131), flat-review tests (from 1827), duplicate.rs's tests (the file has 251 lines), the class map's size (45 pairs), commit file counts (f23a42b1 58, 9aba90be 55).
