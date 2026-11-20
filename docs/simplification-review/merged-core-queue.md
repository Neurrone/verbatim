# Core's event queue and reducer: merged and verified review

Merged from reviewers A and B, every finding checked against main at ba89dca7 (read-only: file reads, grep, wc, git). NVDA is the submodule at 732d1ce2. "Product" lines leave out each file's `#[cfg(test)]` module.

## 1. Size

### Settled table

| Part | Verbatim product | Verbatim tests | NVDA counterpart (files and lines) | NVDA lines | Ratio |
| --- | --- | --- | --- | --- | --- |
| The queue proper: `event_queue/` without `outposts.rs` (driver 1,212, inbox 744, limiter 446, mod 483, pairs 29, request 82, table 563) and `entries.rs` (1,397) | 4,956 | `event_queue/tests.rs` 1,815, `limiter_tests.rs` 689, `entries_tests.rs` 1,832, `window_destroyed_tests.rs` 273, `tests/merge.rs` 499: 5,108 | `queueHandler.py` 1-102 (102); `eventHandler.py` 48-171, queueEvent to `_EventExecuter` (124), and 312-361, executeEvent (50); `IAccessibleHandler/orderedWinEventLimiter.py` 1-128 (128); `IAccessibleHandler/__init__.py` 1019-1105, pumpAll (87); `core.py` 990-1048 CorePump (59), 1180-1203 requestPump (24), 1206-1227 callLater (22); `watchdog.py` 133-256, freeze wait and the foreground-moved recovery test (124), 330-447, cancellable calls (118) | 838 | 5.9x product; tests 5,108 against `tests/unit/test_orderedWinEventLimiter.py` 375 (NVDA has no unit tests of its event queue), 13.6x |
| Of which the limiter (`limiter.rs`) | 446 | 689 + 499 | `orderedWinEventLimiter.py` 1-128 and pumpAll 1019-1105 | 215 | 2.1x |
| Outpost generation record and liveness (`event_queue/outposts.rs` 413, `supervisor/` 476) | 889 | 529 + 183 | none: one process (D9, D13) | 0 | cross-process only |
| Held tokens | 152 | 118 + 238 | none: NVDA holds COM objects by reference | 0 | cross-process only |
| Recorder, flight recorder, dump | 941 | 388 + `replay_fixture.rs` 347 | none (architecture section 9) | 0 | Verbatim-only by decision |
| Foreground record, desktop name, input desktop, applications, shell | 600 | 75 + 234 + `tests/shell.rs` 850 | `eventHandler.py` 340-347 and 362-376 (desktop name, callLater 250); `IAccessibleHandler/__init__.py` 786-814 (desktop switch, callLater 200); `keyboardHandler.py` 740-742 (callLater 30) | about 60 | about 10x |
| Reducer dispatch and state (`reduce.rs` 2,387, `state.rs` 1,144) | 3,531 | `tests/reduce.rs` 3,681, `tests/foreground.rs` 1,301 | shared with the focus area's review, not settled here | | |
| Test harnesses | 0 | `src/desk_tests.rs` 681, `tests/desk/mod.rs` 748, `tests/alloc.rs` 426 | none | | |
| Area total | about 11,200 | about 15,000 (without `focus_speech_tests.rs` and the replay tests) | | | |

The reviewers' NVDA figures: A gave about 730, B about 1,475. A was closer. A cut pumpAll to 1061-1103 (43 of its 87 lines), estimated core.py at 90 and left out executeEvent. B counted whole files: all of `eventHandler.py` 1-599, which includes acceptance (`shouldAcceptEvent` 481-599, the focus area's), speech validity (`FocusLossCancellableSpeechCommand` 186-311) and `doPreGainFocus` 378-431, and all of `watchdog.py` 1-452, its initialise, terminate and observer code included. The settled ratio for like work is 5.9x: A's 7.4x and B's 3.4x were each off by one of these.

What the queue's extra 4,100 lines buy, settled:
- The cross-process hand-off: the request table with request numbers, owners, late answers and stages (`table.rs` 563), the readers map, the generation record (889) and held tokens (152).
- Behaviour Dickson ruled that NVDA lacks, ADR 0003 "A frozen window" (2026-11-13, and 2026-11-16 for the exemptions and the recovery set): due times, the foreground stop, the recovery set and the half-second round, and the exempt entries. ADR 0003 line 81 adds the 150 ms held-key release ("The keyboard stays alive").
- Exact replay of a paused function: the checkpoint, rebuild and `ReplayMismatch`, about 250 lines in `driver.rs`. This is the fourth of ADR 0003's five rules ("Core's code is async functions", 2026-11-14), and the roadmap's "Exact replay of a paused entry" weighs it again.
- About 750 product lines with no requirement behind them, made up of:
  - the Logic layer's mirrors and its 22 default bodies (about 280);
  - four judges of the front's stop (about 100 net);
  - two records of the foreground (about 80);
  - the keys bridge (about 120 in Core);
  - smaller copies (about 170).

### Deletion target

| Part | Product lines | Test lines |
| --- | --- | --- |
| Remove now | about 680 | about 1,000 |
| If Dickson rules for the removal (the concrete queue, with the typed run) | about 340 | about 260 removed, an unknown number added (section 4) |
| Planned phase (M4.9 `proven`; M6.4 model key variants and terminal timer; M6.3 utterance variants; exact replay's `Front.answers`) | about 180 | the terminal and text tests that feed old inputs, replaced |

## 2. Findings

| Id | Title | Class | Found by | File and line | What goes | Product saved | Tests saved | Speech changes |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| F1 | Four hooks judge whether the front's request stops | Remove now | both | `event_queue/mod.rs` 228-275, 392-402; `inbox.rs` 386-450, 452-472, 607-619; `driver.rs` 148-167 | `withdraws`, `stops`, `own_foreground_rule`, `unneeded` and their call sites, replaced by one judge with a cause; `stopped_at_once` merged with `at_once`; `entries::unsettled`'s copy of `unneeded` | about 100 net | about 60 | no |
| F2 | The queue is generic over `Logic`, with mirrored Input, Effect and Outcome | Question | both | `inbox.rs` 19-182; `entries.rs` 1164-1211, 1359-1390; `mod.rs` 199-483 | a concrete queue: the two mirror enums, their translations, the 22 default bodies | about 300 | about 260 removed, builders added | no |
| F3 | Core's state holds a hollow queue while a step runs | Defect | A only | `state.rs` 613-617, 1082; `entries.rs` 1075 | `SrState.queue`, so no function can read an emptied queue | about 0 | 0, one test to add | not shown, needs a test |
| F4 | One foreground report enters Core twice and is kept twice, with the own-window rule ADR 0003 removed | Remove now | both | `verbatim-app/src/main.rs` 1163, 1200; `foreground.rs` 41-43, 53-62; `reduce.rs` 213-216 | `Input::Foreground`, `ForegroundFact::Changed`, `foreground_changed`, `ForegroundWindow.own` and the own-window rule; one record in the queue | about 80 | about 25 | no (pinned by `tests/foreground.rs` 1123 and 1150) |
| F5 | Key presses are rebuilt into the old Input vocabulary; dead variants | Remove now (Core); planned M6.4 and M6.3 (model variants) | both | `keys/entry.rs` 266-324; `reduce.rs` 247-281, 104-108; `verbatim-model/src/event.rs` 661-664, 767 | `today`, `waited_input`, `command_arrived`, `key_arrived`, `Input::Tick`, `Input::MarkReached`, the dead log in `main.rs` 1439-1443; later the eight key variants | about 120 now, about 110 later | replaceable | no |
| F6 | A reader's readiness kept in three places | Remove now | both | `driver.rs` 228-230, 321-334; `inbox.rs` 207-209, 664-666; `record.rs` 116-118 | `Queue.readers`, derived from a listener stage in the record | about 30 | 0 | no |
| F7 | Backoff due time and liveness times written twice | Remove now | A only | `policy.rs` 94-101; `record.rs` 24, 36-40, 52-56, 152-163; `verbatim-supervisor/src/lib.rs` 100-122 | `Backoff.due_us` for applications; one definition of the times | about 20 | 0 | no |
| F8 | The speech fence is a field of every entry kind | Remove now | A only | `entries.rs` 385-431 and 17 constructors | `fence`/`set_fence` arms; a run must keep one fence per candidate | about 40 | 0 | no |
| F9 | A run is `Vec<E>` and drops non-candidates silently | Question (with F2) | A only | `limiter.rs` 109-113; `entries.rs` 510-512, 960-970 | `Item::Run(Vec<Candidate>)`; the unreachable `Entry::Candidate` arm | about 40 | 0 | no |
| F10 | Work proportional to the waiting line on each arrival, pass and entry | Planned: M6.1 (limiter, held tokens); Exact replay (checkpoint clone) | both | `limiter.rs` 262-303, 318-423; `state.rs` 985-995; `driver.rs` 704 | nothing yet: measure under a flood | 0 | add 1 (about 40) | no |
| F11 | `entries::unsettled` re-derives the settle judgement | Remove now | both | `entries.rs` 1317-1330; `reduce.rs` 94-97 | `unsettled`; settle after every non-queue input | about 15 | 0 | no |
| F12 | Two rules for sending a held view | Remove now | A only | `held_tokens.rs` 94-142; `verbatim-app/src/live.rs` 32-47, `main.rs` 1531-1551 | the outposts' rule, the listener's kept | about 40 | 0 | no |
| F13 | Stale documents, `expect(dead_code)` and old-dump fallbacks | Remove now | both | `event_queue/mod.rs` 16-25; `entries.rs` 15-24; `reduce.rs` 207, 258, 413, 550, 665; `shell.rs` 1-12; crate guide 137-141, 389-396; glossary 235; six `#[serde(default)]` | the attributes, stale text and fallbacks | about 50 | 0 | no |
| F14 | A miss defaulted silently in `proven.rs` | Planned: M4.9 | both | `proven.rs` 23-30 | the module, as M4.9 states | about 30 | about 17 | no |
| F15 | Core's own requests begin by four paths; the Follow mapping written twice | Remove now | B only | `driver.rs` 1095-1103; `inbox.rs` 89, 264; `entries.rs` 801-807, 1299-1305 | `Input::Send`, `Cx::send`'s separate path, one mapping | about 40 | 0 | no |
| F16 | `kept_by` restates `told_token` | Remove now | B only | `entries.rs` 732-744; `announcements/mod.rs` 283-292 | both `kept_by` | about 25 | 0 | no |
| F17 | The shell's copy of the listener generation and `GenerationMismatch` | Remove now | B only | `held_tokens.rs` 60, 106-151 | the copy and the error | about 30 | about 20 | no |
| F18 | Every answer formatted as text and kept twice | Planned: Exact replay | B only (A in passing) | `driver.rs` 435, 903 | `Front.answers` and `Shown` | about 25 | 0 | no |
| F19 | The lock key's 30 ms read is a wake outside the queue's | Remove now (lock key); M6.4 (terminal timer) | B only | `verbatim-app/src/main.rs` 1882-1919; `state.rs` 522, 706-719 | the shell's second deadline for the lock key | about 30 | 0 | no |
| F20 | `Live.probing` can be set on a waiting request | Remove now | B only | `table.rs` 50-76 | `probing` moved into `Stage::Out` | about 5 | 0 | no |

**F1 (both).** The declarations are `fn withdraws(...)` (mod.rs 269-275), `fn stops(...)` (249-258), `fn own_foreground_rule(request: &verbatim_model::request::Request) -> bool` (240-243) and `fn unneeded(...)` (395-402). Each is asked at its own point:
- `withdraws` only in `arrive` (inbox.rs 386-401);
- `stops` in `stop_if_ended` (430-450, after an arrival, a foreground report and `Input::Settle`) and in `stops_as_it_begins` (driver.rs 151-167);
- `unneeded` at each `settle` (inbox.rs 463-467, run from `take_next`, driver.rs 685) and again in `entries::unsettled` (entries.rs 1326-1329);
- the foreground stop at inbox.rs 611-613: `(!L::own_foreground_rule(&live.request) && !L::exempt_from_foreground(&state.facts, &front.entry, &live.window) && !foreground.holds(&live.window))`.

`git show bc99bc40` confirms that the dialog-shown wait added `stops`, `own_foreground_rule`, `merged`, `stopped_at_once`, `stops_as_it_begins` and `Table::answer_cancelling`. B's risk holds in the code: at a foreground report the generic stop runs (inbox.rs 617-618) before `stop_if_ended` (652), so a feature's own reason is shadowed unless it also overrides `own_foreground_rule`.

One correction to A's proposal: dropping the begin-time judgement is not equivalent. Today a request stopped as it begins is never sent (driver.rs 113-119 withholds `ready`). Judged after the pause, the request would go out and be cancelled, an extra Send and Cancel across the process. B's form, one `ends(facts, front, request, view, cause)` with causes Begins, Arrived(entry), Foreground and Settled, keeps every predicate at its point. `withdraws` must still see the arriving entry before the limiter merges it (inbox.rs 386-418).

**F2 (both).** Section 4 has the settled facts. Corrections to A: the test accessors at driver.rs 378-412 and table.rs 170-183 do not go with a concrete queue. Grep finds them in 11 feature test files, among them `table_for_tests` in `announcements/tests.rs`, `changes_tests.rs` and `navigator_tests.rs`. A's 600 lines is therefore about 300.

**F3 (A only, confirmed).** `feed` takes the queue out first: `let queue = std::mem::take(&mut state.queue);` (entries.rs 1075). It then lends `std::mem::take(state)` as the facts (1077), so inside a step `cx.facts()` is an `SrState` whose `queue` is `Default`. state.rs 614-615 says so: "Empty while a step of the queue runs, when the driver holds it."

`terminal::commands::answer` and `command_mark` run inside an entry's function (`cx: &mut Cx<Entries>`, commands.rs 141 and 220). They call `held_anchors(&facts, node)` (151, 230), which calls `state.held_anchors()` (77), whose last loop is `for request in self.queue.requests()` (state.rs 1082). Inside the step that loop sees no live request: no terminal output read under `Owner::Facts`, no outside ask. `keep_place` is then handed a set missing those anchors.

Of the other `SrState` methods that read `self.queue`, grep shows only the shell calling them, after a step (`verbatim-app/src/main.rs` 1298, 1419, 1467, 1502, 1520, 1792), so `held_anchors` is the one reached from inside. No spoken difference is shown. The defect is the representable state, and A's fix (state as `{ facts, queue }` at the top) removes it. The terminal caller is M6.4's code; the type is this area's.

**F4 (both).** main.rs 1163 sends `self.apply(Input::Foreground(ForegroundWindow { ..., own: report.process == std::process::id(), ... }))` and main.rs 1200 sends `self.queue(QueueInput::Foreground(report))`, with a ghost mapped to its hung window (1174-1199). The two records are `foreground.rs` 41-43 (`window: Option<ForegroundWindow>`), read only by `hook_foreground` (state.rs 738), and `Queue.foreground` (driver.rs 231-232).

The own-window rule at foreground.rs 59-61 is `if change.own { self.entered = None; }`. ADR 0003 line 106 says: "Verbatim's own windows have no exception: a foreground change into one is like any other". Grep finds `.own` read nowhere else.

The desk harness feeds only `QueueInput::Foreground` (tests/foreground.rs 417-426), so `a_return_from_verbatims_menu_says_the_title` (1123) and `..._with_its_focus_alone_says_the_title` (1150) already pass without the rule. B's "about 30 call sites" is wrong: grep finds three test feeds of `ForegroundWindow` (`tests/shell.rs` 2, `tests/hung_menu_end.rs` 1). The hook's facts (integrity, console thread) move into the queue's report.

**F5 (both).** `fn today(trace_id: TraceId, press: &Press) -> Option<Input>` (keys/entry.rs 268) rebuilds `Input::Command`, `Script`, `HeldKey`, `CaretKey`, `CharacterTyped`, `ClearingKey`, `EditingChord` and `ControlTyped` for `reduce::waited_input` (reduce.rs 261-273). `command_arrived` (252-255) and `key_arrived` (279-281) are one call each, under docs describing the held-key line ("one pressed while a held key waits reaches the application after it").

Grep over `crates/`, `xtask/` and `uia-facts/` outside tests finds no product constructor of the eight key variants: the app only matches `Input::Command` and `Input::Script` for a log (main.rs 1439-1443). Two other variants are never sent:
- `Input::MarkReached` (model event.rs 767): the app sends `QueueInput::MarkReached`, speech_events.rs 67.
- `Input::Tick`: "Unused by M1 logic but part of the frozen vocabulary" (event.rs 661-663); constructed only in tests (flight_dump, dump, replay, e2e artifacts).

The eight key variants are fed straight to `reduce` by `src/terminal/tests.rs` (`typed`, 206), `tests/text.rs`, `tests/terminal_reading.rs` and `tests/terminal_support`, and by test code in `verbatim-replay` and `verbatim-e2e`. Removing them from the model therefore waits for M6.4's terminal tests. `Routed::AtOnce` (entries.rs 1257-1280) is M6.3's (utterance) and M6.4's (terminal timer).

**F6 (both).** `readers: BTreeMap<Reader, Instance>` (driver.rs 228-230) is written at inbox.rs 209 just after `admit_ready` writes `Stage::Ready(running)` (outposts.rs 115-117). For the listener the record keeps only `listener: Option<u64>` (record.rs 118), which is why the map survives. `admits` judges by the record (driver.rs 329-334) and `ready` by the map (321-323). verbatim-app's `Generation::{Starting, Ready}` (live.rs 50-57) is a third copy.

**F7 (A only, confirmed).** `Record::failed` writes both `application.backoff` (via `Backoff::failed`, policy.rs 128, `self.due_us = ...`) and `Stage::WaitingOut { due_us }` (record.rs 160). `backoff_ended` moves the stage only (outposts.rs 342-358), so `backoff.due_us` goes stale for applications, and only the stage is read (`next_due`, record.rs 168-178). The spares still use `Backoff::due` (`verbatim-supervisor/src/owner.rs` 504, 526). `LIVENESS_US: u64 = 9_000_000` and `BACKOFF` (record.rs 24, 36-40) copy `Timings::default()` and `STAYED_UP` (`verbatim-supervisor/src/lib.rs` 100-122).

**F8 (A only, corrected).** `fence()` and `set_fence()` (entries.rs 385-431) match every variant, and 17 product constructors write `Fence::default()`. A's proposal, the fence on the waiting item, would lose per-candidate fences that are read: `validity.rs` 380 `if last.since != Some(candidate.fence)` and `msaa_run.rs` 442. A run must keep one fence per candidate, so the saving is about 40, not 60.

**F9 (A only, confirmed).** `Item::Run(Vec<E>)` (limiter.rs 113) can hold any entry. `run_entry` drops what is not a candidate with `_ => None` (entries.rs 964-967), and `run` keeps an arm for a lone `Entry::Candidate` (510-512). A candidate is always placed in a run (limiter.rs 191-193), so that arm is unreachable. `focus/menus.rs` 293 builds `Entry::Candidate` only for a view. A typed run needs the queue to know the candidate type, so it rides on F2 or on a new associated type.

**F10 (both).** On every MSAA arrival, `limit` walks every waiting item twice (limiter.rs 319-368) and `join_run` walks every run (270-287). On every asking entry, `take_next` clones the whole state, the waiting line and table included: `let base = shared.state.clone();` (driver.rs 704). After every pass, the shell calls `held_tokens` (main.rs 1502), which walks every entry (state.rs 988-995). `tests/alloc.rs` grows only the ancestor chain (`a_focus_step_allocates_no_more_than_in_proportion_to_the_ancestor_chain`, 199).

A cited ADR 0003's "about 1,100 a second". That figure is derived (ADR line 230). The measured figures (design `readers-tell-core-asks.md` 469) are 27 to 33 tells a second with File Explorer idle and about 3,000 to 4,000 in floods. M6.1 lists "Queues an application can fill have no count limit" (roadmap). Measure first, per the standing rule.

**F11 (both).** `unsettled` (entries.rs 1320-1330) re-derives `terminal::reading::wants`, `QueueRead::Wanted` and `unneeded` so that `reduce` (94-97) knows to settle. Settling after every non-queue input adds one queue step to each such input; that cost needs stating if built.

**F12 (A only, confirmed).** The listener's rule, `ListenerView::after_pass`, sends `ViewSend::Now` only when a handled or last-held token is released, and leaves the rest (held_tokens.rs 94-142). The outposts' rule, `Live::held_changed`, sends `HeldChange::Nodes` on any change of the node set, additions included (live.rs 38-39; main.rs 1538-1544). ADR 0003 line 47 says the ids are "released when Core releases them, by the same held view". Taking the listener's rule for both changes message timing, not speech.

**F13 (both).** Confirmed items:
- mod.rs 16-17 "Built beside the reducer: nothing feeds it yet." and entries.rs 15-16, each over a module-wide `expect(dead_code, ...)`. `Core::dump` has no product caller: grep finds `.dump()` only in `event_queue/tests.rs`.
- `crate::focus_queue` cited at reduce.rs 207, 258, 413, 550 and 665; the file does not exist.
- shell.rs 1-12 lists "an application that stopped responding" and "the fake focus after a menu".
- `wedge_decision` and `Parked::other` (crate guide 137-139, glossary 235) exist nowhere.
- The crate guide (392-394) says every step allocates "the same", while `tests/alloc.rs` 199 pins "no more than in proportion" for the focus step (the text and terminal tests do pin "the same").
- Old-dump fallbacks: six bare `#[serde(default)]` (state.rs 608, 611; driver.rs 242; focus/versions.rs 162, 170; terminal.rs 103), the doc at state.rs 502-505, shell.rs 37-38 ("one from a dump made before announcements were kept has nothing to take") and keys/mod.rs 50-51. They go against the crate guide's "keeps no fallbacks for dumps older code wrote" (verbatim-core.md 370). A's "nine" overcounted: foreground.rs 45 and focus_rules.rs 72 and 78 pair `default` with `skip_serializing_if`, which needs it.

**F14 (both; B's lines corrected).** proven.rs 23-30: `let Some(part) = text.get(range.clone()) else { debug_assert!(false, ...); return ""; };`, with 12 call sites. B's "proven.rs 316 to 322" does not exist (the file is 51 lines). `docs/design/lints-and-unsafe.md` 41 already decides it: "this milestone removes every one, and the `proven` modules with them". That is M4.9.

**F15 (B only, confirmed).** Core's own requests begin by four paths:
- `Cx::send`, used once (focus/follow.rs 231);
- `Cx::send_wanted` with `Logic::wanted` (driver.rs 1122-1126, called at execute.rs 572);
- `Input::Send`, used only by `desktop_switched` (entries.rs 1312);
- `Logic::window_destroyed` returning requests (inbox.rs 232).

entries.rs 801-807 and 1299-1305 build the same `CoreRequest { token: None, reader: Reader::Outpost(follow.process), window: follow.window, request: Wire::Follow(follow.follow), due_after_us: OTHER_DUE_US }`.

**F16 (B only, confirmed).** `Announcement::kept_by` (announcements/mod.rs 284-291) is true for exactly the kinds `told_token` returns (entries.rs 468-476), and `Entries::kept_by` for `UiaFocus` compares the same `listener` field. It is `told_token(entry).is_some_and(|(g, _)| g == instance.generation)` for the listener.

**F17 (B only, confirmed).** `ListenerView.generation` (held_tokens.rs 60) exists so that `after_pass` can return `GenerationMismatch` (113-118), documented as "a logic bug, since both learn of a listener's start from the same input". The app logs it at main.rs 1505-1508. This is an agreement check between two copies.

**F18 (B only, confirmed).** `let shown = Shown(format!("{outcome:?}"));` (driver.rs 903) runs for every outcome, in product, and is pushed to `Front.answers` (435, 772). The same ending also goes to `Checkpoint.answers` (776). Text replies are thus formatted on the speech path and serialized into each dump. ADR rule five ("a state dump shows each entry, the answers it has had") needs one of the two copies, and the Exact replay phase decides the checkpoint's fate, so that phase decides which copy stays.

**F19 (B only, confirmed).** The shell reads `toggle_key_read_due()` and `terminal_timer_due()` after each step (main.rs 1884-1919) beside `QueueEffect::WakeAt`. `queue_idle` (state.rs 759-761) leaves out `toggle_keys`, which the shell's `idle_conditions_clear` adds back separately (main.rs 1419-1422).

**F20 (B only, confirmed).** `Live { ..., stage: Stage, probing: Option<u64> }` (table.rs 59-76). `probing` is set only in `send`, where the stage becomes `Out` (92-97), but a `Waiting` request with `probing: Some` can be written.

## 3. Test findings (verbatim-core)

**Removable, about 1,000 lines:**
- `src/desk_tests.rs` (681). Its only user is `focus_speech_tests.rs`. It shares 343 of its 482 distinct lines with `tests/desk/mod.rs` (748; comparison after normalising `pub(crate)`) and 33 of its 35 function names. It differs by feeding `crate::entries::feed` with the queue's `Input` where the other feeds `reduce` with `QueueInput`. Keeping one harness, fed as the shell feeds Core, saves about 600.
- `entries_tests.rs` 1621-1711: the four tests `a_request_from_outside_the_queue_is_not_stopped_by_a_foreground_change`, `..._past_its_due_time_is_cancelled_and_answered`, `..._has_its_window_checked_for_a_hang` and `..._withdrawn_by_its_asker_is_cancelled`. They repeat `event_queue/tests.rs` at a costlier level with `contains` and `any` assertions. Keep 1590, which covers the route. About 90.
- `event_queue/tests.rs`: seven of B's eight outside-the-queue tests, each an ending already tested for the queue's owner through the same `Table::end` and `deliver`: `a_request_outside_the_queue_found_hung_by_the_shell_is_answered_not_responding`, `a_waiting_request_outside_the_queue_past_its_due_time_answers_its_asker_unsent`, `..._waits_for_the_next_reader_when_one_fails`, `..._withdrawn_is_dropped_unsent`, `a_request_outside_the_queue_found_hung_is_cancelled_with_the_sound_and_answered`, `..._past_its_due_time_is_cancelled_and_answered` and `..._answers_reader_ended_to_its_asker`. Keep `..._out_withdrawn_is_cancelled`, the only test of `WithdrawOutside`. Counted: 122 lines; B's 240 was overstated.
- `tests/merge.rs`: the five property tests (356-412) differ only in `PropertyId` behind one merge arm (entries.rs 938-944). Keep one; about 45. B's 300 also counted cutting the sequence loop, which is what lets `a_focus_merged_across_a_change_of_its_line_moves_behind_it` fail, so it stays.
- `tests/reduce.rs` 456-465, `replay_is_deterministic`: the same determinism is pinned by `replay_fixture.rs`. About 10.
- `foreground.rs` unit tests of the own-window rule, with F4. About 25.
- The hook-specific toy overrides that go with F1. About 60.
- `ListenerView`'s mismatch tests, with F17. About 20.

**To fix, 1 test.** `flight_recorder_dump_replays_to_the_same_effects_as_live_reduction` (tests/reduce.rs 468-486) passes `&mut verbatim_core::Runtime::default()` for every live input, so every live step is itself a rebuild and the test cannot fail on a rebuild defect. Keep one runtime across the loop. Its `sample_script` ends with `Input::Tick` (453), which `reduce_input` sends to `shell::reduce` and drops, so that step tests nothing.

**Replaceable:**
- With F2, the 61 queue tests and 18 limiter tests would run on real entries. Section 4 says what that costs.
- The terminal and text tests that feed `Input::Command` and `Input::CharacterTyped` to `reduce` take a path production never takes: no stroke pass, no say-all stop at the turn. They move to presses with M6.4.
- `tests/shell.rs`: the eight one-output tests (start and error 14, exit 37, lock key 22, clock 36, clipboard 28, item gone 15, unclickable 15, previews 53: 220 lines) could be unit tests of `shell.rs`, saving about 140. B's 250 was overstated, and the value is low.

**Weak assertions:** about ten `contains` or `any` assertions (entries_tests.rs six, tests/reduce.rs three, outposts_tests.rs one). Grep finds none in `event_queue/tests.rs`, against A's count of one.

**To add:**
- One allocation test growing the waiting line to 1,000 entries (F10), about 40 lines.
- With no F3 type change, one unit test calling `held_anchors` from inside an entry while a terminal read naming an anchor is out.

**Generated tests:** none in this area. `tests/merge.rs` is a metamorphic test (the merged run against the run without the older tell), not a comparison of the code with itself.

**Not routinely run:** `tests/replay_fixture.rs` `regenerate_fixture` is `#[ignore]`d. It is a regeneration tool; keep it.

## 4. The settled waiting states, the Logic hooks, and the concrete-queue question

### The waiting states (21)

A listed 18 and B 21. A folded the dialog-shown wait into "a request out" and left out say-all's next read and the terminal's quiet period. All three are distinct waits that ride on the queue, so the settled count is 21. Of these, NVDA has a full counterpart for 8, a partial one for 3, and none for 10. B's "9" counted the request out as full.

| Waiting state | What waits, and for what | Every ending, with the function that judges it | Lines and tests the endings cost | NVDA counterpart |
| --- | --- | --- | --- | --- |
| 1. Entry waiting (`Queue.waiting`, `Item::One`; driver.rs 225) | an entry, for the front to free (an event) | its turn: `limiter::take` (limiter.rs 428-446) from `take_next` (driver.rs 683-715). Merged by a newer one: `merge_away` (limiter.rs 235-256) with `Logic::merges` (entries.rs 925-948) and `merged`. A show and a hide of one object cancel (limiter.rs 201-216). Capped: `limit` (318-423). Foreground change: inbox.rs 624-646 with `ends_at_foreground` (entries.rs 972-1012) and `ended_at_foreground` (670-676). Its listener ended: inbox.rs 679-685 with `kept_by` (entries.rs 732-744). Taken by the front's function: `Cx::take_waiting` (driver.rs 943-956, from caret_tell.rs 312 and focus/narrow.rs 38) and `take_waiting_before` (961-1001, from terminal/reading.rs 1317) | about 450 product; `limiter_tests.rs` 18 tests (689), `tests/merge.rs` 9 (499), the foreground and kept-by cases in `entries_tests.rs` | queueHandler.py 1-102 and `_EventExecuter` (eventHandler.py 109-171); the limiter's caches (orderedWinEventLimiter.py 59-80) and flush (83-128). No foreground drop of waiting events: that is ADR 0003, "A frozen window" |
| 2. A run of MSAA candidates (`Item::Run`) | candidates, for their turn, judged newest first | its turn as `Entry::Run` (`take`, limiter.rs 432; `run_entry`, entries.rs 960-970). A foreground tell removes a focus of its object (`join_run`, limiter.rs 270-287). Capped at four, what it carries handed on (403-416). Foreground change per candidate (inbox.rs 632-640) | about 60; limiter tests 9 | yes: pumpAll's focus winEvents newest first (IAccessibleHandler/__init__.py 1061-1092); the limiter's focus cache |
| 3. The menu slot (`Item::Menu { moved }`) | the newest menu tell, for the runs behind it, once | replaced by a newer menu tell (limiter.rs 195-199); moved once behind the last run (433-440); taken (441); foreground change (inbox.rs 625) | about 20; limiter tests 2 | yes: `_lastMenuEvent` (orderedWinEventLimiter.py 76-78, pushed last at 93-95) |
| 4. An entry Core appends later (`Queue.timers`) | the check after a menu, popup or switcher end, for 50 ms (a time) | its time, at `tick` (inbox.rs 721-727); nothing else ends it. `timer_due_near` (driver.rs 1052-1066) joins one pump's ends | about 40; `menus_tests.rs` (17 in all), `tests/hung_menu_end.rs` 4 | yes: `core.callLater(50, _fakeFocus, ...)` (IAccessibleHandler/__init__.py 988) |
| 5. The front paused in its ask (`Front.asked`, `Shared.slot`, `Checkpoint`) | the front's function, for its request's one ending | any ending of rows 6 to 8, through `deliver_probed`'s `Owner::Queue` arm (inbox.rs 528-529) to `resume` (driver.rs 761-783). Rebuilt after a clone or a restored dump by `rebuild` (555-620) | about 330 (rebuild, step, check, poll, resume, `Asking` 1138-1212, `Slot` 837-907); the five-rule tests, 7 in `event_queue/tests.rs` (1230-1440) | none: NVDA's main thread blocks inside the call |
| 6. A request waiting for its reader (`Stage::Waiting`) | a request, for its reader's instance (an event) | the reader's `Ready`: inbox.rs 207-218, `admit_ready` (outposts.rs 108-128), `Table::reader_ready` (table.rs 233-252). Due time: `tick` (inbox.rs 702-707). Foreground change, front only (607-618). Withdrawn by an arrival (386-421). The feature's own stop, or stopped as it begins (driver.rs 112-167). Unneeded (inbox.rs 463-470). The outside asker withdraws (265-270). Its application exits (outposts.rs 213-244, `answer_all`). Its launch fails with the application gone (87-90). Its process is not watched (`want_outposts`, 360-406). A reader that ends leaves it waiting (table.rs 376-396) | about 150; `event_queue/tests.rs` 12 tests, `outposts_tests.rs` 3 | none |
| 7. A request out (`Stage::Out`) | a request, for its answer (an event) or a time | answered (inbox.rs 337-381, table.rs 285-305). Refused as hung before sending (inbox.rs 191-203, table.rs 307-322). Answered with what is in front (294-311, 324-337). Hang check hung (569-576, with `hung`, 492-499). Due time (702-707). Foreground (592-619, `exempt_from_foreground` entries.rs 541-553). Withdrawn by an arrival (inbox.rs 386-425, entries.rs 620-651). The feature's own stop (430-450). Unneeded (452-472). Outside withdrawal (265-270). Reader ended (655-698, table.rs 376-396, outposts.rs 135-168). Application exited (outposts.rs 213-264). Pong lapse or too many abandoned workers (178-207, 299-339). Delivery shared: `stop` (inbox.rs 474-490), `Table::end` (table.rs 254-280), `deliver_probed` (inbox.rs 515-564) | about 600; `event_queue/tests.rs` about 30 tests, `entries_tests.rs` 5, `window_destroyed_tests.rs` 7, `outposts_tests.rs` 5 | in part. `watchdog.py` 206-232 recovers when the foreground has changed, after `MIN_CORE_ALIVE_TIMEOUT = 0.5` (line 47); it cancels by `CoCancelCall` (235-240) and cancellable calls (330-447). `UIAHandler/utils.py` 439-455 drops a hung window's events. `editableText.py` 94-107 ends a caret wait on a waiting script or a pending focus. No due time, no withdrawal by an arrival |
| 8. The listener's wait for a dialog's show (`AwaitShown`, a request out) | the read of a hidden dialog, for the window shown (an event) | shown (answered, `Outcome::Result`). The user elsewhere: `shown::stops`, `LeftHiddenDialog` (focus/shown.rs 201-223), from `stop_if_ended` or as it begins. A key, withdrawn by the same judge, by the speech fence (shown.rs 218). The application exits: `answer_cancelling` (outposts.rs 223-238; table.rs 417-453). Exempt from the foreground stop (`own_foreground_rule`, entries.rs 555-557). Otherwise hung, due time or listener ended, as row 7 | about 265 in the queue (bc99bc40's queue diffs) and 388 in `shown.rs`; `shown_tests.rs` 32 tests (1,077 + 190 in bb65a26b) | none: NVDA gathers a dialog's text when its focus comes (`NVDAObjects/behaviors.py` 93-182) |
| 9. A request no longer awaited (`Table.late`) | the instance's late answer | the late answer, dropped (table.rs 300-303); its instance ends (380); its application exits (402) | about 15; 4 tests (`event_queue/tests.rs` 955-1019) | none |
| 10. Endings given at once (`at_once`, `stopped_at_once`; driver.rs 72, 79) | a plain reader's answer, or a stop held as the request began, for the function to pause | `answer_at_once` (driver.rs 730-757), after the poll pauses (802) and in the feed loop (inbox.rs 278-285); cleared on rebuild (driver.rs 614-615) | about 45; the plain-read case in `event_queue/tests.rs`, `plain_tests.rs`, the begin cases in `shown_tests.rs` | none |
| 11. A request Core's facts await (`Owner::Facts`) | a terminal's read, for its ending | any ending of rows 6 and 7, to `Logic::facts_answered` (inbox.rs 531-552, entries.rs 715-724) | about 35 in the queue; the terminal tests | none: NVDA reads a terminal on its own monitor thread (`behaviors.py` 468, `LiveText._monitor`) |
| 12. The recovery set (`Table.recovery`) | a window refused or stopped as hung while attended, for a check finding it not hung | not hung: inbox.rs 577-584 and `Logic::recovered` (entries.rs 703-709); destroyed (inbox.rs 230); attention lost (602-606); its outpost ends (667-673) | about 60; 6 tests (`event_queue/tests.rs` 1128-1229) | none as a mechanism (ADR 0003: "as NVDA reads its focus live once its call returns") |
| 13. The hang round (`round_due_us`) | the shell's next half-second check (a time) | due at `tick` (table.rs 522-535, inbox.rs 709-712); disarmed when nothing is to check (`settle_round`, table.rs 537-543, from `finish_step`, driver.rs 652) | about 35; 1 test (1075) | in part: the watchdog's 0.5 s (watchdog.py 47) |
| 14. An outpost generation (`record::Stage`: Launching, Starting, Ready, WaitingOut) | an application's outpost, for the owner's report, `Ready`, a pong within 9 s, or its backoff's end | started (outposts.rs 51-68); launch failed (74-92); ready (108-128); ended (135-168); pong (180-207); application exited (213-264); foreground ends a backoff (281-297); liveness or backoff due (303-339, 342-358); wanted by a request (364-406) | 413 + `record.rs` 179 + `Backoff` about 80; `outposts_tests.rs` 14, policy tests 9 | none: one process |
| 15. The listener started and not ready | the listener generation, for `ListenerReady` | ready (inbox.rs 207-209, `admit_ready` outposts.rs 112); its end (137-144); a newer start (38-40) | about 10; `tells_of_an_old_generation_or_listener_are_dropped` | none |
| 16. A held key's stroke (`StrokeLine`) | a caret key's stroke, for its turn or 150 ms | `pass` at its turn (keys/strokes.rs 104); `release_due` at `tick` (163, entries.rs 694); ended at a foreground change (entries.rs 670-676); `drop_unsent` at a desktop switch or `StrokesReset` (strokes.rs 182, entries.rs 1230-1235); `exit` (200) | keys area (strokes.rs 434); 6 + caret key tests | none: ADR 0003 line 81, "The keyboard stays alive" |
| 17. The virtual desktop's name (`desktop_name`) | the name, for the next focus or 250 ms | a focus says it (`name_for_focus`, desktop.rs 56-62); `tick` says it alone (43-52); a newer name replaces it (25-33) | about 50; `tests/shell.rs` 2 | yes: eventHandler.py 347 (`callLater(250, ...)`) and 362-376 |
| 18. The return from another input desktop (`InputDesktop::ReturnPending`) | the focus read, for 200 ms | `tick` appends the read (input_desktop.rs 130-140, via entries.rs 697); a switch away replaces it (94-112) | about 30; `input_desktop_tests.rs` 6, `tests/shell.rs` 2 | yes: IAccessibleHandler/__init__.py 796 (`callLater(200, _handleUserDesktop)`), 809-814 |
| 19. A lock key's announcement (`toggle_keys`, state.rs 522) | the announcement, for the shell's read of the key 30 ms after the press | `LockKeyToggled`, rebuilt from `QueueInput::LockKeyRead` (entries.rs 1274-1279), pops it (shell.rs 37) | about 20 in Core, 20 in the app; `tests/shell.rs` 2 | yes: keyboardHandler.py 740-742 (`callLater(30, ...)`) |
| 20. Say-all's next read (`QueueRead::Wanted`, say_all.rs 98) | the next read, for playback's marks | joins at settle (`Logic::joining`, entries.rs 653-658; say_all.rs 223); withdrawn by a key (entries.rs 650) or unneeded (say_all.rs 234); stopped by a cut | say-all area | yes: a queueHandler generator (queueHandler.py 79-101) |
| 21. The terminal's quiet period (`terminal_timer_due`, state.rs 717) | the terminal's read, for a time | the shell's `TerminalDue`, rebuilt as `Input::TerminalTimer` (entries.rs 1268-1273) | terminal area (M6.4) | in part: `LiveText._monitor`'s stabilize delay (behaviors.py 468-481) |

### The Logic hooks (32 methods and 3 types)

`event_queue/mod.rs` 201-483. A counted 27 methods; there are 32. One implementation, `impl Logic for Entries` (entries.rs 464-1041), overrides all 32. The toy logics override only 10 to 12. The trait's 22 default bodies (`let _ = ...; None` and the like, about 90 lines) therefore exist only for the toy logics.

| Hook | Called at | Real implementation (entries.rs) and the features in it | For one feature? |
| --- | --- | --- | --- |
| `run` | driver.rs 723 | 498-535: every kind's function | no |
| `told_token` | state.rs 993 | 465-492: UI Automation focus; toasts and selections | two |
| `asks_nothing` | driver.rs 703 | 537-539: terminal speech | yes (terminal) |
| `exempt_from_foreground` | inbox.rs 612 | 541-553: acceptance, announcements | two |
| `own_foreground_rule` | inbox.rs 611 | 555-557: `shown::waits` | yes (dialog shown) |
| `stops` | inbox.rs 437, driver.rs 160 | 559-573: `shown::stops` | yes (dialog shown) |
| `merged` | limiter.rs 185, 221, 284, 413 | 575-577: `shown::merged` | yes (dialog shown) |
| `arrival` | inbox.rs 742 | 579-614: UI Automation change, announcements, keys, terminal keys, strokes, and the fence for every entry | no |
| `stamp` | driver.rs 1081 | 616-618: the speech fence | one mechanism |
| `withdraws` | inbox.rs 392 | 620-651: caret key, say-all read | two |
| `joining` | inbox.rs 460 | 653-658: say-all | yes (say-all) |
| `unneeded` | inbox.rs 466, entries.rs 1329 | 660-668: caret key, say-all | two |
| `ended_at_foreground` | inbox.rs 645 | 670-676: held caret key | yes (keys) |
| `due` | driver.rs 536, entries.rs 1395 | 678-686: strokes, input desktop, desktop name | three |
| `tick` | inbox.rs 715 | 688-701: strokes, terminal keys, desktop name, input desktop | four |
| `recovered` | inbox.rs 582 | 703-709: focus re-take, terminal | two |
| `wanted` | inbox.rs 457, driver.rs 1124 | 711-713: terminal | yes (terminal) |
| `facts_answered` | inbox.rs 541 | 715-724: terminal | yes (terminal) |
| `started` | inbox.rs 598 | 726-730: the startup read | yes (focus startup) |
| `kept_by` | inbox.rs 680, 682 | 732-744: UI Automation focus, announcements (restates `told_token`, F16) | two |
| `reader_ended` | inbox.rs 689 | 746-785: follows, terminal, say-all, navigator, start marker, re-take | no |
| `watched` | outposts.rs 283, 385 | 787-789: ignored processes | yes |
| `window_destroyed` | inbox.rs 232 | 791-809: plain reader, follows | two |
| `plain_answer` | driver.rs 107 | 811-813: plain reader | yes |
| `dropped_at_arrival` | inbox.rs 409 | 815-827: console caret tells, menu openings | two |
| `place` | limiter.rs 126, 140, 152, 165, 178; driver.rs 975 | 829-923: every tell kind | no |
| `merges` | limiter.rs 183 | 925-948: UI Automation focus, caret, UI Automation change | three |
| `exempt_from_cap` | limiter.rs 374 | 950-958: readers' last focus | yes (focus) |
| `run_entry` | limiter.rs 432 | 960-970: MSAA run | yes (focus) |
| `ends_at_foreground` | inbox.rs 626, 634 | 972-1012: held caret key, candidates, menus, current focus, MSAA change, caret, announcements | no |
| `probe` | inbox.rs 405 | 1014-1032: UI Automation focus, announcements | two |
| `foreground_marker` | inbox.rs 647 | 1034-1040: focus versions | yes (focus) |

Fourteen hooks serve exactly one feature: `asks_nothing`, `own_foreground_rule`, `stops`, `merged`, `joining`, `ended_at_foreground`, `wanted`, `facts_answered`, `started`, `watched`, `plain_answer`, `exempt_from_cap`, `run_entry` and `foreground_marker` (`stamp` serves one mechanism). The trait began with 4 methods (`run`, `exempt_from_foreground`, `withdraws`, `recovered`; `git show fc5dccbe:crates/verbatim-core/src/event_queue/mod.rs`) and has had 27 commits to `mod.rs` since.

### Why the queue is generic, and what a concrete queue would do (no recommendation)

**Why it is generic.** The queue was written before anything fed it. `fc5dccbe` (2026-11-16), "Add Core's async driver, request table and inbox", says it was "built beside the reducer in verbatim-core's event_queue module; nothing feeds it yet". Its tests port "the prototype's rule tests" from `prototype/core-form-trial` (named at build1 design line 865), run against "a small logic whose entries each ask a chain of property reads" (`event_queue/tests.rs` 1-4).

The module documents state the boundary the trait enforces: "What an entry is and what its function does are the [`Logic`]'s; the queue holds only what crosses features" (mod.rs 14-16). The ruling behind that boundary is "Decided (Dickson, 2026-11-14): each feature's code sits together in its own module, and the queue holds only what crosses features" (`docs/design/readers-tell-core-asks.md` 49; build1 875). That ruling is about where code lives. It does not require a trait, and no document found by grep names the `Logic` trait or genericity as a decision. The trait is the compiler's enforcement of the boundary: the queue's code cannot match on `Entry`'s variants.

**What the toy logics buy.** `Test` (tests.rs 82-272, impl 171-272), `Probing` (1440-1536) and `Limited` (limiter_tests.rs 36-230) give entries that real code cannot make:
- `Entry::Read { reads }` asks an arbitrary chain of reads;
- `Entry::Stray` waits on something other than an ask (`a_function_waiting_on_anything_but_an_ask_is_caught`);
- `Entry::PlainThenRead` mixes the plain reader with an outpost;
- `Withdraws = bool` withdraws on demand;
- `Msaa { accepted }` sets acceptance directly.

They also give `Facts = u32`, so the dump test's expected string ends `facts: 1` (tests.rs 1416-1436). They cover Tables 2, 18 and 1 and the five rules, isolated from focus rules. They do not exercise the 20 hooks added since (`stops`, `unneeded`, `joining`, `wanted`, `arrival`, `kept_by` and so on), which are tested only through `Entries` in the feature tests.

**Lines of the mirrors and translations:**
- `Input<E>` (inbox.rs 19-96): 78 lines.
- `Effect<S>` (125-182): 58 lines.
- `run_input`'s effect and outcome mapping (entries.rs 1359-1390): 32 lines.
- `route`'s copying arms (Answer, RefusedHung, InFront, HangCheck, ReaderReady, ReaderEnded, WindowDestroyed, Due, WithdrawOutside, Process): about 25 lines. Its Tell, Foreground and AskOutside arms parse and would stay in some form.
- The trait's default bodies: about 90 lines.

That totals about 280, or about 300 with the generic parameters. `widen` and `outcome_of` (driver.rs 861-886) and `request.rs`'s `Outcome<T>` serve the typed requests of ADR rule two, not the genericity, and stay either way. B's middle course, a generic queue emitting `QueueEffect` directly (`Effect { Queue(QueueEffect), Said(S) }`), removes the `Effect` mirror and its mapping (about 80) while keeping the toy logics.

**What a concrete queue would do to the tests:**
- The three toy `impl Logic` blocks (about 260 lines) go.
- The 61 queue tests and 18 limiter tests would build real `Entry` values: tells with `TellWindow` facts so that `acceptance::accept` gives the wanted verdict, candidates with `MsaaIds` and `RequestWindow`, and an `SrState`.
- Tests of the five rules need misbehaving functions (`Stray`, an ask let go). These exist only as a `#[cfg(test)]` variant inside the product `Entry`, which every exhaustive match in `entries.rs` (at least nine) would then carry, or those rule tests go.
- The dump test's expected string would carry all of `SrState`.
- Limiter tests would depend on acceptance and `place`'s real rules, so a change there could fail a limiter test.
- The compile-time guarantee that the queue does not judge entries would become a convention.

## 5. Questions for Dickson

**F2, the concrete queue** (Architecture and high-level design, a Dickson area under CLAUDE.md). It changes no speech, feature or decision of record: the 2026-11-14 ruling ("the queue holds only what crosses features") holds either way. Options:
- Keep the generic queue. The about 280 lines of mirrors and default bodies stay, and each queue input or effect is added in four places plus the toy logics.
- Keep it generic but emit `QueueEffect` directly. This saves about 80 and keeps the toy logics.
- Make it concrete over `Entry` and `SrState`. This saves about 300 product and 260 test lines, adds real-entry builders to 79 tests, and needs a test-only `Entry` variant or the loss of the misbehaving-function rule tests. With it the typed run (F9) saves about 40 more.

No other finding changes speech, removes a feature or contradicts a decision of record. F4 removes code that contradicts ADR 0003 line 106.

## 6. Cost of one behaviour, and what would cut it

Checked with `git show --stat`:
- **The dialog-shown wait**, `bc99bc40` (51 files, +3,718) and its review fixes `bb65a26b`. It touched seven of the eight queue files (driver +76, inbox +34, limiter +47, mod +31, outposts +19, request +14, table +44) and `entries.rs` (+26). It added three hooks (`stops`, `own_foreground_rule`, `merged`), `stopped_at_once`, `stops_as_it_begins` and `answer_cancelling`, beside `shown.rs` (388) and `shown_tests.rs` (1,077, then +190).
- **The window destroy**, `bc61c881`. It added `Logic::window_destroyed`, an inbox arm and an `Effect` variant with its mapping, plus a 272-line test file.
- **One menu-end check per pump**, `ecb116ad`. It added `timer_due_near` to the driver.
- **The background-bar gate**, `ccdb2a7a`. It touched `entries.rs`, `mod.rs` and both toy logics' tests.

A new waiting behaviour on the queue today touches:
- a model `QueueInput` or `Request` variant and its `route` arm;
- the queue's `Input` or `Effect` mirror and `run_input`'s mapping;
- an `Entry` variant with arms in at least nine exhaustive matches in `entries.rs` (`fence`, `set_fence`, `tell`, `Debug`, `told_token`, `place`, `probe`, `ends_at_foreground`, `run`);
- usually a new stop hook with its call site;
- often a `CancelReason` or ending arm (`outcome_of`, `Slot::deliver`, the mapping);
- one of the two desk harnesses;
- the toy logics, when a required hook is added.

The shared mechanisms both reviewers propose, merged:
- one judge of the front's request with a cause (F1), so a new ending is a match arm, not a hook and a call site;
- effects the queue emits as `QueueEffect` directly, or a concrete queue (F2, Dickson's);
- per-candidate fences kept, every other fence set once at arrival (F8);
- one wake path, the lock key joining `Logic::due` (F19);
- one desk harness.

With these, the dialog-shown wait would have touched the judge's match, its own module and its tests, not seven queue files.

## 7. Dropped or corrected

Dropped or corrected as claims; their findings stay:
- A, F2: the test accessors at driver.rs 378-412 and table.rs 170-183 would not go; 11 feature test files use them. The 600-line estimate is corrected to about 300.
- A, F1: dropping the begin-time judgement is not equivalent. It would send and then cancel a request that today is never sent.
- A, F8: the fence moved onto the waiting item would lose per-candidate fences read at validity.rs 380 and msaa_run.rs 442.
- A, F13: three of the "nine" `serde(default)` (foreground.rs 45, focus_rules.rs 72, 78) are required by `skip_serializing_if`.
- A, F10: "about 1,100 a second" is ADR 0003's derived figure; the measured figures are 27 to 33 idle and 3,000 to 4,000 in floods.
- A, tests: the one weak assertion counted in `event_queue/tests.rs`; grep finds none.
- A, size: NVDA about 730, corrected to 838 (pumpAll and executeEvent undercounted).
- B, size: NVDA about 1,475, corrected to 838 (whole files counted, acceptance and speech validity included).
- B, L: "proven.rs 316 to 322" does not exist (51-line file); the code is at 23-30.
- B, B: "about 30 call sites" of `Input::Foreground` in tests; grep finds 3.
- B, tests: outside-the-queue removals "240" corrected to 122 lines (7 tests, keeping the `WithdrawOutside` test); merge.rs "300" corrected to about 45, the sequence loop kept; shell.rs "250" corrected to about 140.
- B, waiting states: "NVDA has a counterpart for 9", corrected to 8 full and 3 partial.

No finding was dropped outright, and no quotation failed to match the code.
