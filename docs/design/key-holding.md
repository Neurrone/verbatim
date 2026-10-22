# Holding caret keys as NVDA does: the prototype and its measurements

Status: prototype measured on 2026-10-22; the decision is Dickson's. The prototype is the branch `key-holding` (0c71f83d, the mechanism; 331abdf6, the measurement workloads and the review cursor step), not merged, built from main at 11c0718a, which is the base it is measured against. The evidence is in `target/e2e-evidence/m42-key-holding-2026-10-22/` (its `readme.md` says what each folder holds; `analysis.txt` has every figure below).

Terms used here:

- A held key is a caret-table key that the keyboard hook swallows and Verbatim sends to the application again once it has read the caret, as NVDA's caret scripts do.
- The before read is that read of the caret, made by the focus's outpost (the per-application process that reads it through UIA or MSAA) before the key is passed on.
- A watch is what Core asks the outpost for after a caret key: wait, without blocking, for evidence of what the key did, then answer with the caret. Its baseline is the caret it compares with.
- A key's effect is misjudged when its answer is read from a caret that does not follow from the key: read before the application handled the key, or compared with a caret older than the one the key started from.

## Decision for Dickson

Whether to merge the prototype, making holding the one mechanism for caret keys in text fields and terminals and replacing the per-case fixes of the caret problems that share one cause (`docs/roadmap.md`).

Recommendation: merge it, after two changes that the measurements show are needed and that the prototype does not have, and a third decision of yours:

1. Keys that are not held must not overtake a held key. Today a typed character pressed while a held Backspace's caret is being read reaches the application first, so "ab", Backspace, "c" typed fast can leave "ac" as "abc" with the "c" deleted. NVDA has the same race. The measured hold is short (median 1.4 to 2.0 ms, 99th percentile 5.7 to 32.7 ms), so it is rare, but it corrupts text, so it should be closed: while any held key waits, the hook holds every later key too, and the shell sends them in order. That holds typed characters, which NVDA never holds; they would get no read, only their place in line.
2. The hook must stop holding keys for an elevated foreground window. Verbatim cannot send input to a window of higher integrity (it has no uiAccess privilege), and Windows drops the re-sent key without an error, so a held key there is lost, not merely unspoken. Re-sent keys reaching elevated windows was put out of scope, but the loss of the key itself was not considered; the hook can check the foreground process's integrity level as it decides (a cached answer per foreground window), and pass the key unheld there. Not measured: no elevated window could be opened without a UAC prompt.
3. Your decision: whether the cost below is acceptable. Holding costs up to about 3 ms more from key to first sound at the median (most in Notepad, through UIA), and 8 to 13 wake-ups and 0.7 to 1.5 ms of processor time per held key in a debug build. For that, the first case (select then copy, then Right) went from misjudged in 60 of 60 keys to 0 of 60, with the same words NVDA says.

If you would rather keep the mechanism Verbatim has, holding stays a terminal tool or is dropped, and the first case gets the per-case fix the roadmap proposes (the outpost's reply to a select carries the caret it read after selecting, and Core takes it as its caret). The measurements found no other misjudged key on main (0 of 180 burst keys), so on this evidence the stale-caret problem is limited to carets Verbatim's own actions move.

## The design as built

### The hook

The keys held are Verbatim's whole caret table (`verbatim_input::caret_bindings`): the arrows and their Control forms, Home, End, Page Up and Down, Control+Home and Control+End, each also with Shift, Backspace, Control+Backspace, Delete, Control+Delete, and Control+A. NVDA holds the Shift forms and Control+A only in controls that do not report their own selection changes; the prototype holds them everywhere, keeping one rule. The clearing keys (Escape, Control+C, Control+D, Control+Break), typed characters, Enter, Control+X and Control+V and every other key pass as before, as NVDA never holds them.

How the hook knows the focus is holdable: a `KeyHolding` switch, an atomic flag the reducer's thread sets after every input from Core's focus (`SrState::holds_caret_keys`: the focus is alive and has text, or has a role that may have it, the same test that decides whether a caret key's evidence is watched). The hook reads it with one relaxed load, so it never waits. A key held for a focus that turns out to have no text is passed on at once by Core.

A held key's press is swallowed and reported as `KeyReport::Held` with the hook's press time and the stroke it came from (virtual key, scan code, extended flag); its release is swallowed too, so the application never sees a release without its press. Its effect on speech (the cancel at the press) is unchanged.

Re-sent keys carry `PASSED_ON_TAG` in the injected input's extra information; the hook passes them, and the modifiers sent with them, straight on, deciding and reporting nothing. Keys the end-to-end harness injects carry their stroke numbers there instead, so they are held like the user's (a unit test checks both).

### Core

`Input::HeldKey` asks the focus's outpost for `TextOp::ReadCaret`. On the answer, Core takes the caret read as the focus's caret, as it takes a caret event's report, passes the key on (`Effect::PassHeldKey`), and handles it exactly as a caret key the hook passed itself, pressed one millisecond after the read ended. The key's existing watch therefore runs unchanged, with the read as its baseline in Core and in the outpost, since each takes the newest caret report from before the key's time. There is no polling.

A key is never lost while Verbatim runs: a read that fails, or does not answer within the bound, passes the key on with the hook's time, and its watch runs as on main.

A held key pressed while another's read is in flight waits behind it, and keys are passed on in the order pressed. The waiting key gets its own before read, made once the key ahead of it was passed on, as NVDA's next caret script reads the caret after the first one's key was sent. That read can come before the application has handled the key ahead, so it may be the caret before that key; the alternative, skipping the read, would leave the waiting key judged from the hook's time, as on main. The read is made on the waiting key's own trace (`Effect::TextForKey`), so that its answer and speech belong to that key; without that, the first judge pass found the speech of Right after Control+Home cancelled as Control+Home's.

No branch on trusted caret events. NVDA's one branch is whether a control's caret events are trusted (`caretMovementDetectionUsesEvents`, false for terminals): with it, NVDA waits for an event, without it, it polls the caret. Verbatim's "after" is always the outpost's evidence read, prompted by the application's events and checked against the caret, so text fields and terminals take one path and the prototype needs no branch.

### The outpost

`ReadCaret` is the read a caret event's report makes, with no formatting: one remote-operations round trip through UIA where the window's provider runs them (the existing caret read program in `verbatim-uia-rops`, which already reads the caret, the selection, the caret's line and the line the selection starts in, so it needed no extension), the classic UIA reads otherwise, and the edit control's messages for a classic edit control. It joins the node's remembered caret reports, so the watch takes it as its baseline, and leaves the selection kept for comparison as it was, so the key's answer compares with the same selection as on main. Backspace's character before the caret or the selection's start comes from the line it reads, as Core already worked it out.

Remote operations audit (CLAUDE.md): the before read is one round trip through UIA, the same program as a caret report; nothing reads the caret twice in one place. A possible reduction left for later: the watch's first check, which follows the before read by a few milliseconds and finds no evidence when the application has not yet handled the key, could be skipped when the before read is that recent.

### The shell

The app keeps a held key's stroke by its trace until Core passes it on, then sends it with `SendInput`: the gesture's Shift, Control or Alt that Windows no longer has down are pressed before it and released after, and the key is released first if Windows still has it down, as NVDA's `KeyboardInputGesture.send` does. Each pass-on logs how long the key was held (`verbatim::held_key`, `held_us`).

The bound: the before read is a request with a deadline of 150 milliseconds (`HELD_KEY_BOUND`). Past it the request ends as unanswered and Core passes the key on without the read. The value: above the slowest before read seen in every run here (the longest hold measured was 62.8 ms, in Windows Terminal), and short enough that a key the outpost cannot read for still reaches the application before it feels lost; NVDA's own wait for a caret move is 100 ms. It is a bound on a request, not a sleep: nothing waits on it unless a read is outstanding.

## Costs

- Latency added per held key, from the hook's press to the re-sent key's injection (minimum, P50, P95, P99, maximum, in ms): text box 0.9, 1.4, 4.9, 12.4, 44.3 (1,305 keys); Notepad 1.5, 2.0, 9.0, 12.7, 44.8 (1,305); console host 1.2, 1.6, 3.9, 5.7, 12.7 (396); Windows Terminal 1.3, 1.9, 3.6, 32.7, 62.8 (396).
- Calls: one round trip through UIA per held key (the before read), or the edit control's window messages for a classic edit control, on top of the watch's checks, which are unchanged.
- Wake-ups and processor time (`cargo xtask trace scenario`, debug build, info logging): in the console host's window (13 to 18 s, typing and arrows) holding added 6 percent of wake-ups (3,434 to 3,651) and 3 percent of processor time (578 to 596 ms); in Windows Terminal's, 7 percent (3,496 to 3,734) and 4 percent (610 to 634 ms). The reducer thread took 36 and 44 percent more wake-ups, the outpost's worker 24 and 15 percent more, and the hook thread 20 and 10 percent more, from the injected press and release. An earlier pair of branch traces, before two bug fixes that do not touch this path, gave 17 and 13 percent more wake-ups. From the hook's extra wake-ups, two per held key, these come to roughly 8 to 13 wake-ups and 0.7 to 1.5 ms of processor time per held key. The text box and Notepad traces are not compared: their fixed windows (46 to 54 s) caught different stretches of the workload in the two builds (the base caught the drains between steps, the branch the keys), so their totals measure the windows, not holding.
- Code: about 1,270 lines added on the branch: about 800 in the crates' sources (the hook's in-file unit tests among them), 380 of tests and measurement workload, and 90 of crate documentation.

## The figures side by side

Key to first sound, in ms (minimum, P50, P95, P99, maximum), three runs of each build, base then held. Each figure is from Verbatim's latency line for the key, which starts at the hook's press, so a held key's figure includes its hold.

Text box (Windows Forms, a classic edit control):

- Right Arrow: base 3.1, 5.9, 29.9, 47.2, 47.3; held 3.9, 5.7, 29.7, 37.1, 43.8.
- Left Arrow: base 3.3, 4.7, 31.3, 38.0, 40.8; held 3.8, 5.4, 33.3, 54.7, 66.3.
- Down Arrow at 150 ms: base 3.4, 6.0, 32.1, 33.9, 37.4; held 4.0, 6.0, 29.9, 37.3, 39.2.
- Up Arrow: base 3.3, 5.0, 27.7, 33.6, 33.9; held 3.9, 6.0, 33.4, 34.5, 57.2.
- Down Arrow in bursts at 30 ms: base 3.5, 4.8, 30.2, 30.5, 34.1; held 4.1, 5.7, 30.7, 30.9, 33.7.
- Backspace at 33 ms: base 1.3, 4.9, 28.9, 32.6, 40.6; held 1.4, 6.2, 27.2, 31.7, 33.4.

Windows 11 Notepad:

- Right Arrow: base 3.4, 5.5, 27.5, 55.3, 58.1; held 4.9, 7.6, 31.5, 43.2, 48.1.
- Left Arrow: base 3.5, 4.8, 39.5, 45.5, 67.1; held 5.0, 7.8, 41.5, 47.0, 70.9.
- Down Arrow at 150 ms: base 3.3, 4.8, 30.8, 35.8, 41.6; held 5.0, 7.0, 28.8, 33.9, 34.0.
- Up Arrow: base 3.4, 5.3, 29.5, 36.1, 65.2; held 4.8, 7.2, 31.5, 34.2, 44.5.
- Down Arrow in bursts: base 3.5, 4.6, 26.8, 30.6, 35.4; held 4.9, 7.5, 31.0, 34.4, 34.6.
- Backspace: base 1.2, 4.4, 30.2, 41.4, 45.7; held 1.2, 6.4, 29.4, 33.3, 43.9.

Console host:

- Left Arrow: base 2.0, 3.7, 30.7, 41.0, 41.5; held 3.0, 4.0, 33.0, 37.0, 42.1.
- Right Arrow: base 2.0, 3.1, 46.0, 49.0, 49.3; held 3.0, 4.0, 32.0, 38.0, 38.0.
- Backspace: base 2.0, 3.9, 27.5, 33.0, 35.3; held 3.0, 4.0, 27.9, 33.0, 33.8.

Windows Terminal:

- Left Arrow: base 2.7, 4.0, 81.0, 124.0, 124.0; held 3.9, 5.0, 95.6, 129.0, 139.0.
- Right Arrow: base 3.0, 4.4, 61.0, 62.9, 75.0; held 3.1, 5.0, 59.0, 79.8, 80.0.
- Backspace: base 2.8, 3.5, 15.2, 27.0, 29.0; held 2.8, 5.0, 27.0, 31.0, 33.0.

Escape, which is never held, moved within the runs' spread (console host P50 10.0 to 13.0, Windows Terminal 8.0 to 8.0). The P50 cost of holding is -0.2 to 1.3 ms in the text box, 1.9 to 3.0 ms in Notepad (UIA's round trip), 0.1 to 0.9 ms in the console host and 0.6 to 1.5 ms in Windows Terminal; the P95 and above are set by the audio's period (the steps near 30 ms appear in both builds) and move by about as much between runs of one build as between the builds. The latency figures are integer milliseconds in `latency.csv`, refined from the log's tenths where its line could be matched.

## How often a key's effect was misjudged

The first case, select then copy followed by Right: the review copy's first press (Verbatim+F10) selects "line" from a start marker at the top line's start through the review cursor on "e", and Right follows at once, ten times in each of three runs, in each application. NVDA, captured on 2026-10-22 (NVDA alpha-58022,732d1ce2, the pinned build) in both review cursor scenarios, says the character after the selection and then the selection's end: "a", "N unselected" there.

- Base, text box: 30 of 30 misjudged. The key's answer was read before the edit control handled Right, from the caret Core knew before the selection, and said "l", the character at the selection's start, where a classic edit control's caret is; "line unselected" was never said.
- Base, Notepad: 30 of 30 misjudged. The answer was read before Notepad handled Right and said "space", the character at the selection's end, with the selection still there; "line unselected" was never said.
- Held, both: 0 of 60. Each said "space", then "line unselected", as NVDA does.

On main the selection's own announcement was also said in two pieces, "l selected ine selected", in all 30 Notepad rounds and 3 of the text box's; with holding, never. Not traced further.

The bursts, ten Down Arrows 30 ms apart from the top, three per run: answers whose line is not the line the caret was on when the key was handled, counted from the flight recorder. Base: 0 of 180 answers (90 in each application), every one spoken. Held: 0 of 171 answers read (the last key of nine Notepad bursts was answered after Verbatim was idle and the recorder dumped; the latency lines show all 90 reached audio). Each key's answer came within the 30 ms before the next key in both builds, so these bursts never made a key wait behind another's read, and never made main misjudge either.

The scenario step: `text_box_review_cursor` and `notepad_review_cursor` end with Right after the copy's first press, expecting "a" then "N unselected" from the capture; both pass on the branch. Main also passes that step once, since there Right comes after the selection's announcement has been heard in full and the application's report of the selection has reached Core; the measure workload presses Right at once.

## Tests and scenarios on the branch

- Unit: the hook's hold decision on and off and the two tags (`verbatim-input-windows`); Core's held key read then passed on and judged from the read, and held keys passed on in order whatever their reads answer, each read on its own key's trace (`crates/verbatim-core/tests/text.rs`); the modifiers of a gesture (`verbatim-app`).
- mockapp: `a_held_keys_watch_is_judged_from_the_caret_read_before_it`: the application moves its caret without reporting it, the before read finds it, and the watch with that baseline stays open until the key's own move rather than answering at once from the focus's report; the whole `caret_watch` suite passes.
- Live, against a local agent, once each with the final build: `text_box_editing`, `notepad_editing`, `text_box_selection_keys`, `notepad_selection_keys`, `text_box_review_cursor`, `notepad_review_cursor`, `conhost_editing`, `windows_terminal_editing`, `terminal_key_timing`, all passing. The first pass found two prototype bugs, both fixed before the figures above: a Shift selection key re-sent without Shift (a normalized gesture name's parts are sorted, so the key is not last), and the Control+Home then Right case, where the waiting Right's read carried Control+Home's trace and its speech was cancelled as an earlier key's.
- `cargo xtask ci` passes on the branch.

## What the prototype did not cover

- Elevated windows: out of scope by Dickson's decision, with the consequence above that a held key is lost there.
- Keys not held overtaking a held one (decision 1 above).
- A stalled reducer thread now delays caret keys in text fields until it recovers, where on main the keys reach the application at once whatever Verbatim is doing; the bound covers only the outpost's read. If Verbatim exits while keys are held, those keys are lost.
- The foreground changing between a key's press and its pass-on sends the key to the new foreground window, as NVDA's would.
- A key auto-repeating is held press by press, so the application receives separate presses rather than repeats.
- The bound was never reached in these runs, so the path that passes a key on without its read ran only in the unit test.
- The waiting key's before read can precede the application's handling of the key ahead of it; no run here made a key wait.
- No release-build figures; all are debug, as the measuring guide's key workloads are.
- The text field traces are not comparable (above); a hand-made trace over a fixed number of keys would settle their cost.
- The other cases of the shared cause (a say-all caret move the application refuses; a selection reported after the next key) were not measured.
- NVDA's settings on this machine have delayed character descriptions on, contrary to its defaults, so its captures say "Alfa" after "a"; the expectations leave that out, as the comparison guide requires NVDA's defaults.
