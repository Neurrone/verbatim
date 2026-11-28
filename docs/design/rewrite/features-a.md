# Features against design-v2: focus, announcements, navigation, review, reports, keys, say-all, shell list

Claude, 2026-11-28. Read-only check against main at 671aa993. Sources: `books/dev/src/reference/test-coverage.md` (rows named below), `docs/parity/`, ADR 0003, `books/dev/src/explanation/design/key-holding.md`, and the code cited. Stage names are ones I propose. The design's own record names are used: `Peers`, `Application`, `Generation`/`Out`, `Queue`, `Versions`, `Foreground`, `Features`, `Keys`, `Settings`; in readers `ListenerRoot`/`ListenedApp`, `OutpostRoot`/`Element`/`Request`.

## Assessment first

- Most features fit. Each is a queue entry that asks the front's one request, or a record with a due time, and the requests are batches from today's step list. None needs a new message kind.
- Three things in today's code have no place in the design as written. Each one touches many features:
  - **Requests owned by a record rather than the front entry.** Rule 1 says "the front entry alone may have a request out". That is Dickson's rule. Today Core also sends requests outside the queue:
    - `Logic::wanted` and `facts_answered` (`crates/verbatim-core/src/wants.rs`) carry the classic progress-bar read and the focused terminal's reads.
    - `Input::AskOutside` (`event_queue/inbox.rs` 80) carries the tray and taskbar walks and the tree dump.
    - `Logic::window_destroyed` sends a `Follow` naming nothing.

    A bar read outside the queue is ruled: "so no key waits behind it", in `progress.rs` 14 to 20 (Dickson, 2026-11-21). The holders of a standing scope (navigator, start marker, say-all, and the readers' last focus at the listener) also need a standing request whose single ending reaches a record, not an entry.
  - **Ending the front's request because of something that is not a foreground change.** Today `Logic::ends` judges the front for `Cause::Arrived` (a later key, or a focus that counts as pending) and `Cause::Settled` (a fact changed, such as the 150 ms release of a held key). The design's `Entry` trait keeps only `foreground_changed`, and rule 5 names only "the due time, a foreground stop".
  - **Scopes that are not a queue entry, a one-shot request or the focus's `Follow`.** Today Core computes these by hand in `SrState::held_nodes`, `held_anchors`, `held_told` and `held_tokens` (`crates/verbatim-core/src/state.rs` 957 to 1060). Besides the focus and its line, they hold:
    - the navigator's node and review anchors;
    - the start marker;
    - say-all's node and positions;
    - a waiting caret key's node bound at its press;
    - an outpost caret tell's element;
    - the readers' last focus's listener token, past its entry.

    The design deletes this computation (correctly), but gives no standing kind that holds ids without subscribing to events.

## Additions the design needs

The verdicts below refer to these by the letters A to H.

- **A. Record-owned requests.** A `Waiter` whose owner is a `Features` record or the plain reader, not an entry, answered into that record's transition. This includes one-shot requests (a bar read, the shell-list walks, a terminal's output reads) and standing ones (the scope holders in C). Dickson decides it, because it changes his rule 1. The other way keeps rule 1 and makes each such read a queue entry. That changes ruled behaviour: `no_key_waits_behind_a_bar_read`, and the tray walk "not stopped by a foreground change" while the list dialog closes (`entries_tests.rs` 1891).
- **B. Ending the front for an arrival or a fact change.** Either a fourth hook on the front (`behind(&self, arriving, &View) -> Keep | End(reason)`), or `Arrival` returning `Join { end_front: Option<reason> }`. Fact changes (the held key's timer, a commit) need `cx`-free access to the `Out` record's cancel from a transition. Dickson decides it: a new queue hook is item 4 in CLAUDE.md.
- **C. A standing scope with no subscription.** It holds ids minted into it until it is cancelled or its peer ends. Two forms would do:
  - A `Follow` with an empty property set, if its definition is widened to allow one. Claude can decide that if `Follow`'s meaning allows it. I recommend it.
  - A new `Keep` step. That is item 4.

  This scope is needed at outposts (navigator, start marker, say-all, review position) and at the listener (the readers' last focus's token).
- **D. Entry scopes for any standing request, and for waiting entries that name outpost ids.**
  - `ScopeId::Entry { watch, item }` must take any standing request's number, so that an outpost's caret tell, sent as a `Follow` item, holds its element until its entry ends.
  - A waiting caret key bound at its press to the focus node must keep that node after the next commit cancels the old `Follow`, as `a_key_whose_field_is_no_longer_the_focus_runs_on_that_field` requires. Today `held_nodes` keeps it through `queue.entries()`.

  The fix: the commit's `Cancel` of the old follow is held back while a waiting entry names a node in it, or the key's entry scope takes a membership through `ended`'s companion field ("adopt"). Either is a protocol change, item 4.
- **E. A polling state in the outpost's `Request`.** Today the outpost's waiting table (`crates/verbatim-outpost-logic/src/watch.rs`) runs `WaitForCaret`: checks fall due about every 10 ms, a UI Automation event of the followed node within 60 ms brings a check forward, and `Cancel` withdraws the wait. The design's `Request` has `Queued | Running | Following`, and a worker job cannot be interrupted. It needs a `Polling { batch, next_check, bound }` variant that drives one job per check with `WakeAt`. Claude can decide this, since no message or step changes.
- **F. The listener's pending foreground.** `crates/verbatim-listener-logic/src/foreground.rs` holds every MSAA tell behind a foreground not yet in front, until `GetForegroundWindow` names it or `FOREGROUND_HOLD_US` passes, and also tracks the consoles' thread watch. Neither appears in `ListenerRoot`. It needs `foreground: PendingForeground` in `ListenerRoot`, with the `GetForegroundWindow` read as a value the callback caches or as a job. Claude can decide this; it is ruled behaviour (ADR 0001).
- **G. Core-side readers and live reads by the shell.** These are all ruled in ADR 0003, "A frozen window":
  - The plain reader (`focus/plain.rs`, "answers every kind of request at once") is a peer in Core with no pipe.
  - The hang check before sending, and every half second while a request is out, is a shell step whose result is a shell input. It is not a pass through the `Io` trait.
  - The menu check's live read of the window in front, and the lock key's state read 30 ms after the press, ride on the wake as shell-read inputs.

  Claude can decide these, as mechanics of the effects and inputs the design already lists.
- **H. `arrive` needs Core's facts.** Today `dropped_at_arrival`, `place`, `exempt_from_cap` and `arrival` read the facts and stamp the entry with them: the newest key number (the fence), Alt down, the setting at arrival, whether the element is on the focus line as it arrives, and the readers' last focus's ids for the cap's exemption. The design's `arrive(&self, &QueueView)` sees only the queue. The facts can be computed when the tell is parsed into the entry, since Core's step has `&Core` there, or `QueueView` can be widened to a read view. Claude can decide this, since no hook is added.

## Records the design deletes, and where each feature's state goes

| Today (`state.rs` and others) | Feature | In the redesign |
|---|---|---|
| `held_tokens` (readers' last focus token) | UI Automation duplicate test (`focus/duplicate.rs` `uia_duplicate` reads `checked.kept` for that token) | A standing scope at the listener (C), opened at commit and cancelled when the last focus changes or empties |
| `held_nodes`: navigator | Object navigation, review | The navigator's own scope at its outpost (C, A) |
| `held_nodes`, `held_anchors`: start marker, say-all | Select then copy, say-all | Their own scopes (C, A); the scope's `Ended` ends the record, as `say_all_stops_when_its_outpost_generation_ends` requires |
| `held_nodes`: a waiting caret key's bound node | Caret keys | D |
| `held_told` (outpost caret tells) | Selecting with Shift, caret tells | D (entry scope on a `Follow` item) |
| `wants`, `bar_reads` | Classic progress bars | A; `bar_reads` becomes the progress record in `Features` |
| `Input::AskOutside` | Tray and taskbar list | A |
| `follows` | Focus follow | `Application`, as the design says |
| `run_executed: bool` | Menu check | One state value in the menu record in `Features` (no flag) |
| `alt_down`, `arrivals`, `strokes`, `toggle_keys`, `typed_word` | Switcher, fences, held keys, lock keys, word echo | `Keys` (`typed_word` in the echo record) |
| `desktop` (input desktop, lock state) | Secure desktop | `Foreground` |
| `desktop_name`, `last_toast`, `cell_memory`, `last_tree_level`, `progress_reported`, `start_marker`, `say_all`, `navigator`, `plain`, `ignored` | Several | `Features` (`plain` as the plain peer, G; `ignored` in `Settings`) |
| `Logic` hooks: `recovered`, `reader_ended`, `started`, `tick`, `joining`, `foreground_marker`, `stamp` | Re-take, startup, timers, say-all's next read, foreground marker, the speech fence | Transitions of the peer records, `Foreground` and `Features` that push entries. `stamp` becomes the driver stamping each `Say` with the running entry's fence, which is the entry's own data |
| `Logic::ends` with `Arrived` and `Settled` | Caret keys, the confirmation, the menu check | B |

## Focus and foreground

- **UI Automation focus** (Duplicate and ordered focus events; The ancestors Core already knows). A listener tell, checked on the check worker, becomes an `Event` item.
  - Entry `UiaFocus`, with these stages:
    - `focus.walk`: a `Batch` to the outpost with `Ancestors` and `Read`, and a `Follow` opened in the batch, with `mint_into: Standing(follow)`.
    - `focus.listener` when the listener reads.
  - It reads `Versions` (the readers' last focus) and the backend cache in `Application`. It writes `Versions` at commit, then `Cancel`s the old follow.
  - The duplicate test needs the last focus's token held at the listener (C).
  - Behaviour: `docs/parity/focus-events.md`, "Duplicate focus suppression", and `focus-context.md`, "Focus-ancestry context".
  - Verdict: fits as designed for the slice, with C added for the token.
- **MSAA focus, runs and the limiter.**
  - `arrive` returns `Merge(run)` for a candidate. `Join` must carry today's `Place`: key, class, accepted, arrival time (`event_queue/limiter.rs` 79).
  - The queue's general placement keeps the caps (`FOCUS_CAP` 4, `THREAD_CAP` 10, `PUMP_WINDOW_US`), the merge that keeps its place, and the menu slot.
  - The run's `run` tries candidates newest first, one `focus.candidate` ask each. A candidate not yet confirmed is a `focus.confirm` batch.
  - The cap's exemption reads the readers' last focus's ids (H).
  - Behaviour: `backends.md`, "MSAA winevent flood control", and `focus-events.md`, "Duplicate focus suppression".
  - Verdict: fits with H, if the limiter moves whole into `Queue` placement as the design says. No hook is added.
- **Focus validity** (`focus/validity.rs`). It stays one pure function over the facts and the waiting entries. "Pending" reads `QueueView` at a decision, after a wait, as rule 4 allows. Verdict: fits.
- **Hidden dialogs shown later** (A dialog read once it is shown; `focus/shown.rs`).
  - The postponement is a state value of `Versions`, or a record in `Features`.
  - A listener `Event` for the show, or a foreground item, pushes a `ShownRead` entry with stage `focus.current`. A commit or the dialog's destruction ends the postponement.
  - A key other than a caret key reads the dialog first under the older fence, which is stamped on the entry (H).
  - "the outpost ending postpones again" is the generation-end transition.
  - Behaviour: `focus-context.md`, "A dialog's text".
  - Verdict: fits.
- **Menus and the menu check** (`focus/menus.rs`).
  - A menu end makes the menu record (today's `run_executed` plus the noted focus) set a 50 ms `Due`.
  - At the wake, the shell reads what is live in front (G). The transition pushes `MenuCheck`, with stage `menu.check`: a `Batch` of the current focus with the focused fact, to the outpost of that window's application.
  - An answer with a ghost in front, or with the desktop window, goes to the plain peer (G).
  - "A focus committed before the check ends it with no read": the entry reads `Versions` at its turn.
  - Behaviour: `focus-events.md`, "The focus after a menu or the switcher closes".
  - Verdict: fits with G.
- **Desktop return and the virtual desktop's name.**
  - The return is a `Watch` item. The `Foreground` transition pushes a `Returned` read with stage `focus.current`, spoken with no duplicate test.
  - The name is a `Features` record with a `Due`. The next focus entry takes it before its containers.
  - Behaviour: `system.md`, "Desktop switches".
  - Verdict: fits.
- **Secure-desktop switch** (`input_desktop.rs`).
  - A switch away:
    - commits the sleeping focus (a `Versions` state with no scope);
    - `Cancel`s the follow;
    - drops the held keys in `Keys`, with a shell effect so the hook forgets its modifiers.
  - The return sets a 200 ms `Due` in `Foreground`, then pushes a read.
  - "No re-take meanwhile" is a guard in the generation-end transition.
  - Behaviour: `system.md`, "Desktop switches" and "The lock screen".
  - Verdict: fits.
- **Re-takes after an outpost restart** (`focus/retake.rs`).
  - The `Serve`'s `Final(Ended)` consumes the `Generation`, and every request in `out` is answered `Ended`.
  - The committed focus becomes a state that keeps its copied data with no scope. The transition pushes a `Retake` entry with stage `focus.current`, sent once the new generation is `Ready`. The read is judged against the kept copy.
  - The same happens when the listener ends, and for `Recovered`, the hung-window set in `Application`.
  - Behaviour: `focus-events.md`, "Recovery after an outpost is replaced", and `review.md`, "Review after a silent re-take", which is pending Dickson.
  - Verdict: fits. Item 3's "elements of an abandoned worker lost" makes the same re-take path serve a foreground abandon.
- **Startup.** The `Listener` record's `Ready` transition pushes the startup read (today's `started`). Verdict: fits.
- **Alt+Tab switcher, shell staging windows, File Explorer, Settings app, table cells, tree level.**
  - The rules stay pure in Core.
  - Alt down at arrival is stamped from `Keys` (H).
  - `cell_memory` and `last_tree_level` go to `Features`.
  - The switcher's end shares the menu check.
  - Verdict: fits.

## Announcements

- **State, name, value and description changes** (`changes.rs`).
  - An outpost's change is a `Follow` item, `Changed`. A listener's UI Automation property change is a `Watch` item.
  - Entry `Change` reads `Versions`: "on the focus line as it arrived" is stamped at construction (H), and "the focus in flight counting" comes from `QueueView`.
  - "A state change read without its states answered is refused": the `change.read` batch.
  - Stale value changes after a report of the focus: an arrival stamp.
  - Behaviour: `changes-and-selection.md`, "State changes and the focus line", "State changes after the focus has left", "A description change" and "Value change on focused node speaks bare value".
  - Verdict: fits with H.
- **Selections and controlled lists.**
  - Entry `Selection` asks once with stage `selection.read` (the controller-for or value facts).
  - Its token is in the entry's scope. It is judged by the committed focus as it arrived (H).
  - Behaviour: `changes-and-selection.md`, "Selection announcements" and "Selection in a list the focus controls".
  - Verdict: fits.
- **Toasts.**
  - A listener `Event`. Entry `Toast` asks for a listener read with stage `toast.read`, with the token in the entry scope.
  - `foreground_changed` returns `Keep`, as the exemption in ADR 0003 requires.
  - The repeat within a second is judged against `Features.toast`, today's `last_toast`.
  - Behaviour: `alerts.md`, "Toasts".
  - Verdict: fits.
- **UI Automation notifications.** A tell with no token, so no scope and no ask. It is judged by the focus's application. Behaviour: `alerts.md`, "UIA notification events". Verdict: fits.
- **MSAA alerts and help balloons.**
  - `Alert` asks one batch with stage `alert.read` (an ancestor test, then a snapshot with descendants).
  - "Moved once behind the pending entries": `cx.apply` pushes the entry again with `moved`, as today's `cx.requeue` does (`announcements/alert.rs`). The entry is exempt from the foreground stop.
  - Behaviour: `alerts.md`, "Other alerts" and "Help balloons".
  - Verdict: fits.
- **Progress bars.**
  - A UI Automation bar plays from its tell at its turn. A flood merges into one entry. Verdict: fits.
  - A classic bar is read outside the queue, with one read out per bar, and is read again when marked. Verdict: **does not fit rule 1** without A. The ruling is in `progress.rs` 14 to 30, and the behaviour in `changes-and-selection.md` and `backends.md`, "MSAA winevent flood control".
- **A value change accepted by the background setting.** The setting at arrival is stamped (H), and `foreground_changed` returns `Keep`. Verdict: fits.
- **Desktop name.** See above. Verdict: fits.
- **Live regions.** Not built: `alerts.md`, "Live regions", is "not yet (M7)". Nothing to map.

## Object navigation, review and reports

- **Parent, siblings, first child, To focus, Report current object, Activate** (`navigator.rs`).
  - Entry `Navigate` asks a `Batch` with stage `nav.move` from the navigator's node. The first child is reached by the route of the committed line when the outpost does not hold the ancestor.
  - The node reached must outlive the request, so it is minted into the navigator's standing scope (C). The navigator record in `Features` owns it (A), and `Cancel`s the old one when the navigator moves.
  - On the focus, the navigator uses the follow's ids, with no scope of its own.
  - Activate is one batch, with stage `nav.activate` (1,024 parents, in the outpost).
  - Report current object reads live with stage `nav.report`. Spelling and copying are shell effects counted by repeat in `Keys`.
  - The silent re-take keeps the navigator on the focus (`review.md`, "Review after a silent re-take", pending Dickson).
  - Behaviour: `review.md`, "Parent/next/previous/first-child moves with edge reporting", "Report current object" and "Object activation".
  - Verdict: fits with A and C.
- **Review commands outside terminals** (lines, words, characters, top and bottom, pages, selection start and end, caret and review location).
  - Entry `Review`, with stages `review.read` and `review.move`, asks a text batch from the anchor of the review position.
  - The anchors live in the navigator's scope, or the follow's when the review follows the caret. Following the caret reads `Versions`.
  - Behaviour: `review.md`, "Review cursor line/word/character over object text" and "Review words by the provider's word".
  - Verdict: fits with C.
- **Flat review.** Its positions are Core's own (`a_flat_start_marker_holds_no_anchor_in_the_outpost`). Its reads are ordinary. Verdict: fits.
- **Set and move to the start marker, Select then copy.**
  - The marker lives in `Features`, with its anchors in its own standing scope (C, A). It survives focus moves (`moving_to_the_start_marker_in_another_object_is_no_start_marker_and_keeps_it`).
  - The copy is a batch with stage `copy.read`, plus a shell effect.
  - Behaviour: `review.md`, "Select then copy from the review cursor".
  - Verdict: fits with A and C.
- **Report focus** (`report_focus.rs`). Entry `ReportFocus` makes a live read of the committed node with stage `report.focus`, through the follow's scope. "Not there", or the hang check (G), says nothing; nothing committed says "No focus". Behaviour: `review.md`, "Report focus". Verdict: fits.
- **Report time and date.** No ask. The repeat count comes from `Keys`, and the shell formats. Verdict: fits.
- **Report status bar** exists only for terminals (`terminal_reading`), so it is outside these areas. **Report title** is not a Verbatim command: no `ReportTitle` in `verbatim-model/src/event.rs`. Nothing to map.

## Keys and gestures

- **The key line and held keys.**
  - The hook holds keys by a view Core publishes, read with one lock-free load (`key-holding.md`, "The hook"). That stays a listed shell exception in the ratchet.
  - `Keys` holds the stroke line. Each held key's 150 ms release is a `Due` in `Keys`, whose transition sends strokes (a shell effect) and must withdraw the front's starting read or watch when it is that key's (B).
  - At exit, Core names the held strokes for the shell to release.
  - Behaviour: `text.md`, "Holding caret keys", and ADR 0003, "Keys and commands".
  - Verdict: fits with B.
- **Caret keys, Control+Backspace, Enter, Backspace and Delete** (`keys/caret_key.rs`).
  - Entry `CaretKey` waits in these stages:
    - `caret.start`: the starting read. It mints the starting point into the key's scope.
    - `caret.watch`: a `WaitForCaret` batch.
    - `caret.confirm`: the focus's facts.
  - The stroke is sent between stages as a shell effect.
  - "A later key or a counting focus withdraws the watch" needs B. The poll and the 60 ms event rule need E. The node bound at the press needs D.
  - Behaviour: `text.md`, "Caret keys and what each speaks", "What Backspace and Delete say" and "Enter in a text field"; `focus-events.md`, "While a focus is pending".
  - Verdict: fits with B, D and E.
- **Auto-repeat counting, repeated presses, the Verbatim key, passing keys, the menu guard, lock keys.**
  - These stay in `verbatim-input` and the hook's own root.
  - The lock keys' 30 ms read is G.
  - Behaviour: `input.md`, "NVDA-modifier semantics" and "Script repeat counting"; `review.md`, "Toggle key announcements".
  - Verdict: fits.
- **The speech-cancel rules** (fence, cut by a modifier).
  - The fence comes from `Keys` and is stamped on entries at construction (H).
  - The driver stamps every `Say` with its entry's fence, in place of `Logic::stamp`. The speech thread keeps its own record.
  - Behaviour: `text.md`, "Speech cut off by a modifier"; `focus-events.md`, "When speech is cut off".
  - Verdict: fits with H.
- **Elevated windows.** A view published to the hook, as today. Verdict: fits.

## Say-all (line and sentence, from the caret and from review)

- The say-all record in `Features` holds the node, the queued marks and the buffer.
- Each read is a `SayAllRead` entry with stage `sayall.read`, pushed by the record's transition at a speech-mark shell input. This is today's `joining`.
- The positions read must outlive each read, so they go in say-all's standing scope (C). The record owns that scope (A), and its `Ended`, when the outpost generation ends, ends say-all.
- A key's cut, through the fence, stops it, and withdraws a read out through B when the read is the front.
- Behaviour: `speech-audio.md`, "Say-all" and "Say-all speaking without pauses"; `review.md`, "Say-all from the caret where there is no text".
- Verdict: fits with A, B and C.

## Mouse tracking

Not built: `docs/parity/input.md`, "Mouse tracking and touch interaction", is "not yet (unscheduled)". The only mouse case is the MSAA pointer-shape name change (`backends.md`), which is dropped. Nothing to map.

## Tray and shell item list (Verbatim+F11)

- `ShellList` shows the GUI list (`shell.rs` 91). The GUI asks Core for the walks with `AskOutside`, and Core sends them outside the queue to Explorer's outpost. The outpost queues them behind the queue's requests (`waiting.rs` 1 to 8). The request is not stopped by a foreground change.
- In the redesign:
  - The shell-list record in `Features` owns three one-shot batches (A): `shell.walk`, `shell.position` and the click.
  - The answers reach the GUI as a shell effect.
  - The click's failures are said queued, as an entry.
- Behaviour: `system.md`, "The system tray and taskbar list".
- Verdict: **does not fit rule 1** without A. The outpost's "outside the queue goes behind" ordering also needs a field in `Ask` or a rule in the outpost's `Request` queue, which is item 4 if it is a field.

## Additions and misfits, listed

1. A, record-owned requests. Dickson decides (rule 1). Needed by classic progress bars, the tray and taskbar list, the navigator, the start marker, say-all, and the window-destroyed `Follow`, which could become a plain `Cancel` instead.
2. B, ending the front for an arrival or a fact change. Dickson decides (item 4, a queue hook). Needed by caret keys, the confirmation, held-key release and say-all.
3. C, a standing scope with no subscription. Claude decides if it is a widened `Follow`; Dickson decides if it is a new step. Needed by the navigator, review, the start marker, say-all, and the listener's last-focus token.
4. D, entry scopes on any standing request, and for waiting keys' bound nodes. Dickson decides (item 4). Needed by caret keys and caret tells.
5. E, a polling `Request` state in the outpost. Claude decides. Needed by caret keys.
6. F, the listener's pending foreground in `ListenerRoot`. Claude decides. Needed by foreground ordering.
7. G, the plain peer in Core, the hang check as a shell step, and live reads carried on wakes. Claude decides. Needed by menus, ghosts, report focus and lock keys.
8. H, facts computed when an entry is built, or a widened `QueueView`. Claude decides. Needed by fences, Alt, the setting at arrival, the focus line at arrival, and the cap's exemption.
9. The one outright misfit of rule 1 as written: classic progress bars and the shell list. They fit only with A, or by changing behaviour that Dickson ruled on 2026-11-21.

Unmapped: none from these areas. Report title, mouse tracking and live regions are not built. The terminal rows (status bar, command marks) belong to another brief.
