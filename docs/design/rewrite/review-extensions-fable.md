# Adversarial review of extensions-design.md

Reviewer: Claude (Fable), 2026-11-28. Read-only; nothing built or run. "The design" is `extensions-design.md`; "the skeleton" is `design.md`; "the additions" is `extension-api-additions.md`. Line numbers for NVDA are in `nvda/source/` at the pinned commit.

## Assessment

The request shape (a call as a standing request, reads through Core, instance records in the owner's record) is sound, and host crash and outpost end do fall out of the skeleton. The design fails in three places it does not name: the lane, which is a queue Core cannot see and which lets a call that "does not hold" hold the event queue; the end causes, which cancel a holding hook in exactly the cases NVDA's Explorer and Calculator hooks exist for; and the speech-text stage, which holds speech no module registered for and silences the dialog needed to turn it off. The trust model has one real hole: the context carries other applications' content past the boundary Core enforces on reads.

## Must-fix

### M1. A call that "does not hold the queue" holds it through the lane

Where: section 6, "A lane runs one call at a time ... Calls Core sends while one runs wait in `calls`"; section 10, "A command call ... does not hold the queue: later keys and events go on"; section 11, no bound on a handler's time.

Sequence: an app module has a command (or a following hook) that waits on the network or a slow read for ten seconds. While it runs, a dialog opens in that application by itself. Its focus matches the module's holding `on-focus` registration. Core sends `Call`; it waits in the lane's `calls` behind the running command. The focus entry is the queue's front and holds every later event. Nothing ends it until the user presses a key or another focus arrives, so the dialog is silent for ten seconds. The command had no end cause and no bound, so neither has the hold.

Core has the fact (the record's `owned` is not empty) but the design does not use it, and question 9 ("calls queue on the host's lane, not in Core") hides the order from the one process that decides what waits. Either a holding call must not queue behind an owned call (a second lane, or the owned call suspended at its next import), or the design must state this hold and its limit.

### M2. `on-node` has no rule for reads that are not the front entry's, and deadlocks on its own module's reads

Where: section 8, "`on-node` ... called for each batch of controls Core reads in the module's applications ... returning changes that apply wherever the control goes"; section 9 lists it as holding; section 11 gives it end causes that only a front entry has.

Sequence one: Calculator's module has the `on-node` registration for nameless history items. A command or following hook of the same module reads the history list's descendants. The answer holds matching nodes, so Core owes an `on-node` call to the same instance. Its lane is `Suspended` on that very read, so the call waits in `calls`, and the `Resume` waits on the call. An owned call has no end cause, waiting is not counted by the breaker, and no bound exists: the lane is stuck for good.

Sequence two: a record-owned read (progress, the shell list, the navigator, another module's `Wants`) returns a matching node. A record's answer "is delivered in `step`, not by `await`" (skeleton, "Requests a record owns"), so there is nowhere for the `on-node` call to wait. Every record owner would need a waiting state.

NVDA's `event_NVDAObject_init` is re-entrant (calculator.py 52 to 59 builds child objects inside it). The design must say which reads `on-node` applies to, and that a module's own reads either skip it or cannot match it.

### M3. The end causes cancel a holding hook exactly when NVDA's hooks act, and "as if passed" is the wrong default for a hook that suppresses

Where: section 11's table gives `on-focus` and holding `on-event` the causes `LaterKey`, `PendingFocus`, `Committed`. The skeleton (line 383): "A cause that held before the stage began therefore ends it at once." Section 12 gives the hook `pending.focus`.

Sequence one: Explorer's menu-bar exit (explorer.py 428 to 441) and the notification area (line 111) drop a focus because `isPendingEvents("gainFocus")` is true. In the design, a focus is pending, so `PendingFocus` holds as `ext.call` begins; Core cancels the call and "goes on with the default as if the hook had passed". The hook never runs with `pending.focus` true, and the menu item is announced. Section 20 lists both as hooks reading `pending`; they cannot work under section 11.

Sequence two: Calculator's `DisplayUpdated` hook is waiting on its read after Enter. The focus moves to the equals button, so a focus is pending. The call is cancelled and the default speaks the notification's display string, which is what calculator.py 123 to 140 exists to replace (the truncated string of issue 16573). No key is involved, so the key fence does not hide it.

The cancelled outcome needs to be chosen per hook (pass or drop), or `PendingFocus` must not be a cause for hooks that read `pending`.

### M4. A call's ending when its instance ends is not given by construction, and the instance's life is not its request's life

Where: section 6, the instance table and "The host's root".

- The row "The instance's `Final` (`Trapped`, `Stuck`)" resolves "every owned ticket". A holding hook's call is the front entry's, not in `owned`, and may be waiting in the lane's `calls`. Nothing says the host answers it or Core resolves it. The same gap is in the rows for the application's exit and for leaving `Active`: Core cancels the `Instance` request, but a `Call` is a separate standing request, not opened inside it, so the skeleton's "Cancel names the batch and every standing request opened inside it" does not reach it. A front entry that sent `Resume` to a cancelled lane waits for an item that never comes.
- "Abandoned ... a new worker generation starts with a fresh instance, the lane's `Instance` request ends `Ended(Stuck)`": both cannot be true. If the request ends, Core's record goes to `Closed` and the next call opens another instance.
- Section 11: a forced cancellation "re-created [the instance] with its state lost" under the same `Instance` request. So the claim "an instance has one ending, its request's" (section 6) is false: the instance ended and the request did not, and Core's `held` still names ids the new instance has no handle for.
- `Ended(Replaced)` is a listed `Final` with no row in the table.
- Reload (section 7): "cancelled ... at its running call's `Final`; the next call opens an instance of the new version". With no bound on a call, the old instance can run for minutes while new calls are due. The record has one `Open` variant, so "open, replacement owed" is a hidden state, and Core needs to know the lane is running an entry-owned call, which the record does not hold.

### M5. Core's `held` and the host's handle table drift apart at every cancellation

Where: section 6, `Released(ids)` is an `Effect` item on a call; section 11, "Core resolves the ticket at once; anything the call sends after that is dropped as late", "the lane drops what it sends".

Sequence: a holding hook holds three handles and is suspended in a read. A later key ends it. The guest gets `cancelled`, returns, and drops its handles. The `Released` item belongs to a call whose ticket is resolved, so it is dropped. Core's record keeps naming the three ids, the held view keeps them, and the outpost keeps three COM objects until the application exits. This happens on every fast Tab through a hooked control. A forced re-creation loses every handle the same way. It is one fact (what the instance holds) kept in two places with the only update path cut by the cancel. `Released` needs a path that is not a call's item, or the instance's `held` must be rebuilt at each call's end.

### M6. The speech-text stage breaks the hot-path promise and silences its own recovery

Where: section 9, "an utterance waiting for its stage holds the utterances after it in the speech order"; the ruling "Events no extension handles keep the budget"; question 5's last item.

- Sequence: a translator takes announcements only. A focus announcement goes to it and waits two seconds. Fifty milliseconds later a terminal's output, a toast or a progress value composes an utterance of a kind the stage does not take. It waits behind the first. That event had no registration and paid 1,950 ms. With a stage that never answers it pays without limit, and the next key press drops it unheard.
- Verbatim's own interface is speech too. With the service down, every announcement in the Extensions dialog becomes "Translation failed" (not counted by the breaker, section 6), and with a stage that hangs it is silence. The option "nothing but cancel speech and the Extensions dialog" cannot work, and the recommended "after N seconds" makes each item of that dialog cost N seconds. The ruling "the original is never spoken" cannot hold for the speech needed to turn the stage off; that speech needs to bypass stages, or a gesture must.
- "Whatever cuts the utterance (... a newer focus's interruption) cancels the call": interruption is the speech root's fact (skeleton, `Settings`: "the speech root owns them"). Utterances waiting before `Say` are a second speech queue in Core, with the cut rules judged in two places. The design lists no state for it.

### M7. The context carries other applications' content past the boundary

Where: section 12, "Every call carries the context ... Core's committed focus and its ancestors; the foreground"; the additions, lines 58 to 79; section 5, "Core enforces the application boundary, since every read goes through Core".

Sequence: the music-player example registers a following `name-change` hook. The player changes its track name in the background while the focus is in a password manager or a banking page. The call's context holds the focus's snapshot with its values and its ancestors' names. The install dialog said only "Example Player: changes how Verbatim reads it". With a network import that is content leaving that the dialog does not show. The context must be filtered by the instance's applications as reads are.

Two more ways past the dialog:

- A global module can declare `pass-through = true` commands on every character key (Calculator's digit pattern, section 10). `on-command` then sees every key in every application, password fields included. Typed characters were kept out of hooks for the hot path; this brings them back with no capability row beyond a long Commands list (`books/extensions/src/reference/capabilities.md` 15 and 27).
- The lock-screen rule covers nodes Core "gives". It does not cover handles an instance already holds from before the lock, a call already running, or a background event whose context subject and `speak` effect put a hidden window's text into speech on the lock screen. The rule has to be on every read's send, every context and every effect.

### M8. Matching a registration is not free, and the worked example's "no read" is wrong

Where: section 8, "one naming others is judged on Core's own read, so it costs only what Core already reads"; section 19, File Explorer, "Cost: no read, one crossing"; the additions line 586.

Evidence: the tab-switch matcher names `parent-automation-id`. Today that value is a live call into the application, made only for switcher candidates on a focus (`crates/verbatim-listener-logic/src/check.rs` 123 to 125 and 353 to 357). It is in no cached set. So either every UI Automation selection event in Explorer, one per arrow key in a file list, pays that call to judge a registration it will not match, or the matcher is evaluated lazily after `role = "tab"`, which the design does not say. `speak-node` "as Core announces a focus" also needs name, role, states and position, which a selection tell is not shown to hold. The design needs a load-time rule: a hook's matcher may name only what the tell for that kind caches, or the manifest states the read and the ledger charges it to every event of that kind.

## Should-fix

1. `ext.ready` (section 7) waits on a record's state, not a request, so it has no ticket. Its ending on `Failed`, or on the host dying mid-compile, is not stated. Gesture bindings exist only in `Active`, so on a cold start Verbatim+T and Verbatim+Tab are unbound for the compile's seconds, and what the key does then is not stated.
2. Forced cancellation destroys state in ordinary use: a key that lands while a translator parses a response wipes its cache, and one that lands while Calculator's hook computes wipes the flag. It depends on epoch timing, so replay must model it. Letting the guest run to its next import under the compute bound loses nothing, since Core's ticket is already resolved.
3. "Only native code ... can do this" (section 6) is unsupported: a host import blocked in the system (a data-folder write, name resolution) is not reached by epoch interruption either. During the 1 second grace the lane takes calls it cannot run.
4. Effects with work of their own have no owner: `click` and `speak-node` need a read after the call may have ended; `copy-text` checks afterwards; `clipboard-text` needs an answer, but `Resume` answers only a `Wants` of shared steps. That is more than "two new owners", and the clipboard answer is a new kind for Dickson's item 4.
5. The built-in host's end "counts against no built-in". A built-in that exhausts the process memory cap then restarts the host forever. Per-store limits would make it a trap that names the module.
6. Report title, Report focus and the tray lists read any application. The roadmap puts "global modules reading every application" in M13 (`docs/roadmap.md`, about line 2010) and ADR 0003 line 249 leaves the exception to this design, which does not decide it.
7. State not in the tables: the breaker's failure times, `Active` with a reload in flight, the instance's replacement owed, the utterances waiting before `Say`, provenance per property. The count of added lifecycle states is understated.
8. Parity gaps:
   - Explorer: section 20 omits `event_UIA_window_windowOpen` (explorer.py 485 to 497), which re-sends the event on a child in another application (no verdict does that, and the boundary refuses the read), `ExplorerToolTip` (116 to 152, mouse time and state shared between two classes), the status bar, and six overlay classes (158 to 213).
   - Section 20 makes the duplicate list-view focus a hook reading `pending`; the additions (section 3) say that comparison is not offered. NVDA judges it before queueing (56 to 72); a hook after the commit leaves the duplicate counted as pending for others.
   - Calculator: `event_nameChange` (72 to 92) handles every name change, so it cannot be registered narrowly; each digit costs a command call, one or more holding name-change calls and the notification's three crossings, where section 19 counts only the last. NVDA clears the flag on any notification (99 to 100), the design only on `DisplayUpdated`. A find from the foreground is refused, because Calculator's frame belongs to `ApplicationFrameHost`.
   - Start search (`searchui.py` 32 to 43): a focus veto that needs three parent reads before queueing.
9. Costs left out of section 18: an instance's first call (instantiation), an installed host's process start under a held focus, and the reads the SDK makes silently for `not-asked`.
10. Sizes: the table omits the package installer, the cache, the breaker, the container and caps in the supervisor, the `wasi:http` allowlist, the secret store, the generators and the simulator's host and network world. Tests at 4,000 lines for 11,000 are thin against the contract tests section 17 lists. "Other app modules about 600" is twice NVDA's 312 (`wc -l` of the five files). The 3,000-line NVDA base (confirmed by `wc -l`) leaves out the add-on store and its dialogs, about 6,900 lines, which are the install dialog's counterpart.

## What held up

- Host crash mid-call, the outpost generation's end, and a cancel crossing a late answer follow the skeleton's `Out` and `Cancel(Late)` rules.
- The sixteen kinds cover every `def event_` in the eight modules to be ported; the kinds left out (`foreground`, `loseFocus`, `caret`, `suggestionsClosed`, `pageChange`) are used only by modules not on the list.
- Calculator's digit-before-notification order holds while both ride the queue's order onto one lane.

## Not verified

- Whether the key fence drops an event's default speech when a later key ended its hook, and whether Core's own focus handling already drops a focus with another pending. M3's first sequence rests on NVDA's comment that the limiter does not.
- What a selection tell caches beyond `check.rs`; Wasmtime's and `wasmtime-wasi`'s behaviour for blocking imports, cancellation and instantiation cost; the AppContainer's network capabilities (loopback and private networks must stay closed for the host allowlist to mean anything).
- The systray add-on, which is outside the pinned source.
- Sections 1, 2 and 4 to 7 of the additions, `docs/design/extensions.md` and the Extensions guide were searched, not read whole.
