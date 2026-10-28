# Holding caret keys as NVDA does

Status: adopted and built (Dickson, 2026-10-22), the one mechanism for caret keys in text fields and terminals. This document keeps the problem, the choices and their reasons, the costs and the limitations. The mechanism is described in the crate guides: the hook in `docs/crates/verbatim-input-windows.md` ("Holding caret keys"), the caret table in `docs/crates/verbatim-input.md`, Core's handling in `docs/crates/verbatim-core.md`, the shell's line of keys and its bound in `docs/crates/verbatim-app.md`, and the before read and the watches in `docs/crates/verbatim-outpost.md`.

Terms used here:

- A held key is a caret-table key that the keyboard hook swallows and Verbatim sends to the application again once it has read the caret, as NVDA's caret scripts do.
- The before read is that read of the caret, made by the focus's outpost (the per-application process that reads it through UIA or MSAA) before the key is passed on.
- A watch is what Core asks the outpost for after a caret key: wait, without blocking, for evidence of what the key did, then answer with the caret. Its baseline is the caret it compares with.
- A key's effect is misjudged when its answer is read from a caret that does not follow from the key: read before the application handled the key, or compared with a caret older than the one the key started from.

## The problem

Without holding, a caret key reaches the application at once, and Verbatim judges its effect from a caret it reads afterwards. When the application has not yet handled the key, or has not yet reported the key before it, the answer is read from the wrong caret: select then copy followed at once by Right Arrow was misjudged in every round in the Windows Forms text box and Notepad (30 of 30 each), and Shift+Right followed at once by Right in 56 of 60 rounds ("How often a key's effect was misjudged"). These cases share one cause, which per-case fixes had treated one at a time. NVDA avoids it by holding the key: its caret script reads the caret, sends the key, and waits for the caret to move from that read.

## Decisions

- Holding is Verbatim's one mechanism for caret keys, replacing the per-case fixes of the caret problems that share one cause, at the cost measured below: up to about 3 ms more from key to first sound at the median, and 8 to 13 wake-ups and 0.7 to 1.5 ms of processor time per held key in a debug build (Dickson, 2026-10-22).
- Keys that are not held never overtake a held key (Dickson, 2026-10-22): see "Keys behind a held key". This goes one step beyond NVDA, which has the same race: there a typed character pressed while a held Backspace's caret is read reaches the application first, so "ab", Backspace, "c" typed fast leaves "ab", the "c" deleted instead of the "b".
- Nothing is held while an elevated window is in front, a temporary guard until Verbatim has uiAccess (Dickson, 2026-10-22): see "Elevated windows".
- The foreground changing between a key's press and its pass-on is an accepted limitation, too small a window to matter in practice (Dickson, 2026-10-22).
- A held key waits for the key ahead of it to be answered no longer than 100 ms after that key was sent on (Dickson, 2026-10-22): see "Core".
- The lock keys are never held back behind a held key (Dickson, 2026-10-26): see "Keys behind a held key".
- In the console host only, a caret key's watch that finds no move on its first check re-reads the caret every 5 ms until a check finds the evidence or 50 ms after the key was passed on, whichever is first, a documented workaround to waiting only for evidence (Dickson, 2026-10-26): see "The console host's caret poll".

## The design

### The hook

The keys held are Verbatim's whole caret table (`verbatim_input::caret_bindings`): the arrows and their Control forms, Home, End, Page Up and Down, Control+Home and Control+End, each also with Shift, Backspace and Delete alone and with Control or Shift, Alt with Up and Down Arrow, and Control+A. NVDA holds the Shift forms and Control+A only in controls that do not report their own selection changes; Verbatim holds them everywhere, keeping one rule. Typed characters, Enter, the clearing keys, cut and paste and every other key are not held, as NVDA never holds them.

The hook decides from a switch the reducer's thread sets from Core's focus (the focus has text, or a role that may have it, the same test that decides whether a caret key's evidence is watched), read with one relaxed load, so the hook never waits. A key held for a focus that turns out to have no text is passed on at once by Core. A held key's release is swallowed too, so the application never sees a release without its press.

### Keys behind a held key

While any key held back has not yet reached the application, the hook defers every later key transition it would pass, typed characters, Enter, clearing keys, modifiers and releases included, so that nothing overtakes the held key; a later caret key is held in its own turn. A deferred key is never read: it is held only for its place in line, and the shell sends it right after the keys ahead of it, in one `SendInput`, which Windows never interleaves with other input. Core keeps the same order, handling a key queued behind a held key once the keys ahead were passed on, each on its own trace so that its answers and speech are its own.

Verbatim's own commands, the guard for elevated windows and the lock keys are the exceptions. Caps Lock, Num Lock and Scroll Lock pass at once, because the router announces a lock key's new state from Windows' own state 30 ms after the press, which a deferred key would not yet have changed, and Windows turns a numpad key into an arrow or a digit by Num Lock's state before the hook sees it, so numpad 4 pressed behind a deferred Num Lock would arrive as Left Arrow. The consequence is in "Limitations".

### Elevated windows

Verbatim runs without the uiAccess privilege, and Windows drops input it sends to a window of higher integrity without an error, so a key held for such a window would be lost. While the foreground window's process has higher integrity than Verbatim's, or its integrity cannot be read, the hook therefore holds and defers nothing. The focus listener reads each foreground window's integrity, and Core publishes whether keys may be held there in a lock-free view the hook applies only while the window it names is still in front (architecture section 5; `docs/crates/verbatim-input-windows.md`, "Holding caret keys"). This is a temporary guard, removed when uiAccess is built (`docs/roadmap.md`, M8.1). The live case is not checked, since no elevated window can be opened without a UAC prompt.

### Core

A held key asks the focus's outpost for the before read. On the answer, Core takes the caret read as the focus's caret, handles the key as a caret key pressed just after the read, and passes it on, so the key's watch runs unchanged with the read as its baseline. The watch is asked for before the key is passed on, so it reaches the outpost before the key reaches the application; with the key passed on first, the console host's output after Up Arrow and a character typed at once was read before the character arrived in 4 of 10 runs of `conhost_up_typing`, and in none of 6 with the watch first. A key is never lost while Verbatim runs: a read that fails, or does not answer within the bound, passes the key on, and its watch runs as without holding.

A held key pressed while another's read is in flight, or while the watch of the held key ahead is unanswered, waits behind it. It gets its own before read once that watch has been answered, with its evidence or without, or once 100 milliseconds have passed since the key ahead was passed on (`KEY_AHEAD_BOUND_MS`), whichever comes first.

This wait is Verbatim's own. NVDA does not wait: its caret script stops waiting for evidence the moment another script is queued and then says nothing, and the next script reads the caret at once (`_hasCaretMoved` returns when `isScriptWaiting()`, and `_caretScriptPostMovedHelper` returns before speaking, `nvda/source/editableText.py` 94 to 95 and 166 to 167). Verbatim waits so that the next read sees the application after the key ahead: reading the waiting key as soon as the key ahead was passed on let the read and the waiting key reach the application before the key ahead was handled, and Shift+Right followed at once by Right was then misjudged in 10 of 60 rounds, where waiting for the answer misjudged none. The bound is NVDA's default wait for a caret to move (`caretMoveTimeoutMs`, 100, `nvda/source/config/configSpec.py` 364), which NVDA multiplies by 3 in Windows Terminal (`nvda/source/NVDAObjects/UIA/winConsoleUIA.py` 470 and 486); Verbatim's is 100 ms everywhere. Without a bound, a key that moves nothing, such as Control+Home at the top or Right at the end of a field, gets no answer until the watch's own bound of 10 seconds, and every key behind it waits that long. The bound adds no wake-up source: its time is kept beside the reducer thread's other deadlines, and the thread wakes for it only when a held key has started waiting.

No branch on trusted caret events. NVDA's one branch is whether a control's caret events are trusted (`caretMovementDetectionUsesEvents`, false for terminals): with it, NVDA waits for an event, without it, it polls the caret. Verbatim's "after" is always the outpost's evidence read, prompted by the application's events and checked against the caret, so text fields and terminals take one path. The one exception is the console host, whose caret events come only on its blink timer ("The console host's caret poll").

### The outpost

The before read is the read a caret event's report makes, with no formatting: one remote-operations round trip through UIA where the window's provider runs them, the classic UIA reads otherwise, and the edit control's messages for a classic edit control. It joins the node's remembered caret reports, so the watch takes it as its baseline, and leaves the kept selection as it was, so the key's answer compares with the same selection as without holding. In a focused terminal it is a read of the terminal, so the reading rules compare the key's effect with the screen just before the key (`docs/design/terminal-model.md`, "Keys").

Remote operations audit (CLAUDE.md): the before read is one round trip through UIA, the same program as a caret report; nothing reads the caret twice in one place. A possible reduction: the watch's first check, which follows the before read by a few milliseconds and finds no evidence when the application has not yet handled the key, could be skipped when the before read is that recent.

### The console host's caret poll

A documented workaround to the rule that a watch waits only for evidence (Dickson, 2026-10-26). In the console host, a caret key's watch that finds no evidence on its first check re-reads the caret every 5 ms until a check finds the evidence, its own re-read or the check an event makes, the console's caret event (`EVENT_CONSOLE_CARET`) included, or 50 ms after the key was passed on, whichever is first; a watch whose last re-read finds no evidence ends as a watch ends at its own bound, and nothing is said for it.

Why: the console host raises its caret events only on its cursor's 530 ms blink timer, one tick covering every move since the last, and nothing earlier follows a caret move: no text change, console update, property change or focus change (`experiments/the-console-host-raises-its-caret-events-on-its-blink-timer/`, 120 Left and Right Arrow presses each in cmd and Windows PowerShell). The caret's signals came 257 to 278 ms after the key at the median and at most 540, while the cursor itself had moved at the median within 1 ms and at most 54 ms after the key. The watch's first check, asked for before the key is passed on, always comes before the console host has handled the key, so without the poll every key that moved only the caret waited for the next tick of the blink timer. The bound is half NVDA's: NVDA does not use caret events in consoles (`_get_caretMovementDetectionUsesEvents` returns false, `nvda/source/NVDAObjects/behaviors.py` 527 to 530) and polls the caret every 10 ms until its 100 ms timeout (`nvda/source/editableText.py` 70 and 150 to 160). A key whose caret moves later than the bound is still spoken: the console's caret event that follows reads the terminal, and the reading rules say what moved.

Rules of the poll, each with its reason:

- Only the focused console host's text area polls; Windows Terminal, whose caret events are timely, and every other text field do not.
- The worker makes each re-read itself, woken by its intake's wait on a high-resolution timer, because a wait at the system's timer resolution, 15.6 ms by default, would miss most reads, and because a wake time only the worker sets cannot be overwritten by another thread's.
- A console caret event for the watched node does not stop the poll when its check finds no evidence, because a blink tick left over from an earlier cursor set, such as `PSReadLine`'s set that moves nothing, can come after the key and before its move.
- A re-read is matched to its watch by the request, not the node, so a worker the watchdog abandoned during a re-read leaves its replacement's watch alone.

Costs, per caret key the first check finds unmoved: at most 10 more remote operations and 10 more wake-ups of the worker, fewer when the caret moves sooner. A key that moves nothing, such as Left Arrow at the start of the line, pays all 10, and its watch is answered after 50 ms, so the next held key waits for it at most that long instead of 100 ms. Measured on 2026-10-26, release builds, `measure_keys_conhost`, the latency from the last key to speech of a one-character announcement after a caret key or Backspace: without the poll P50 3.4, P95 44.0, P99 146.8 and maximum 233.1 ms, the watches left open by the first check answered 84 ms after opening at the median and 434 at most; with it P50 3.5 to 3.6, P99 10.0 to 10.7 and maximum 11.6 to 13.3 ms over three runs, every such watch answered within 8 ms and none reaching the bound. Caret reads by watches went from 1.32 to 1.34 per key, since one re-read, or the console update's check that comes as soon, found each move. The typing echo did not change.

### The shell

The app keeps a held key's stroke in its line until Core passes it on, then sends it with `SendInput`, pressing before it the gesture's Shift, Control or Alt that Windows does not have down and releasing them after, as NVDA's `KeyboardInputGesture.send` does. Since every transition after a held key is deferred behind it, the modifiers are as they were when it was pressed. Verbatim is not idle while any key waits in the line, since that key has not reached its application (`docs/design/communication.md`, section 6.7).

The bound: the before read is a request with a deadline of 150 milliseconds (`HELD_KEY_BOUND`); past it the request ends as unanswered and Core passes the key on without the read. The value is above the slowest before read seen (65.5 ms, in Windows Terminal), and short enough that a key the outpost cannot read for still reaches the application before it feels lost; NVDA's default wait for a caret move is 100 ms. It is a bound on a request, not a sleep: nothing waits on it unless a read is outstanding.

## Costs

Measured on debug builds, before keys were ordered behind a held key; the wait for the key ahead is measured in "A key held with auto-repeat".

- Latency added per held key, from the hook's press to the re-sent key's injection (minimum, P50, P95, P99, maximum, in ms): text box 0.9, 1.4, 4.9, 12.4, 44.3 (1,305 keys); Notepad 1.5, 2.0, 9.0, 12.7, 44.8 (1,305); console host 1.2, 1.6, 3.9, 5.7, 12.7 (396); Windows Terminal 1.3, 1.9, 3.6, 32.7, 62.8 (396).
- Key to first sound, three runs of each build: the P50 cost of holding is -0.2 to 1.3 ms in the text box, 1.9 to 3.0 ms in Notepad (UIA's round trip), 0.1 to 0.9 ms in the console host and 0.6 to 1.5 ms in Windows Terminal. The P95 and above are set by the audio's period and move by about as much between runs of one build as between the builds.
- Calls: one round trip through UIA per held key, or the edit control's window messages for a classic edit control (1 UIA call remotely, 8 classically, 5 window messages, pinned by `caret_moves_cost_exactly`), on top of the watch's checks, which are unchanged.
- Wake-ups and processor time (`cargo xtask trace scenario`, debug build, info logging): in the console host's window holding added 6 percent of wake-ups and 3 percent of processor time, in Windows Terminal's 7 and 4 percent. The reducer thread took 36 and 44 percent more wake-ups, the outpost's worker 24 and 15 percent more, and the hook thread 20 and 10 percent more, from the injected press and release: roughly 8 to 13 wake-ups and 0.7 to 1.5 ms of processor time per held key. The text field traces are not compared, since their fixed windows caught different stretches of the workload in the two builds.

## A key held with auto-repeat

The one case where the wait for the key ahead repeats is a key that moves nothing, held with auto-repeat: every repeat waits for the one ahead of it. `measure_held_repeat_text_box` measures it: three times, Control+End in the Windows Forms text box, then Right Arrow held about a second as Windows' default auto-repeat sends it (the press, then 15 repeats from 500 ms, 33 ms apart, and the release at 995 ms). The last repeat reached the application 1,047 to 1,064 ms after the release; from the first repeat on, each was passed on about 110 ms after the one ahead of it (the 100 ms bound, then the read), so the backlog grew by about 77 ms per repeat. Nothing was said after the release, since Right Arrow at the end of the text moves nothing. The press itself and the first repeat, 500 ms later and so past the bound, waited for nothing. Without the bound each repeat waited out the watch ahead of it, 10 seconds, and the last reached the application about 149 seconds after the release.

## How often a key's effect was misjudged

Measured with three runs of ten rounds of each case, the build before holding against the build with it.

- Select then copy followed at once by Right: the review copy's first press (Verbatim+F10) selects "line", and Right follows at once. NVDA (captured on 2026-10-22, alpha-58022, the pinned build) says the character after the selection, then the selection's end: "a", "N unselected". Without holding, 30 of 30 misjudged in the text box (the answer read before the edit control handled Right, saying "l") and 30 of 30 in Notepad (saying "space" with the selection still there); "line unselected" was never said. With holding, 0 of 60, each saying "space", then "line unselected", as NVDA does. `text_box_review_cursor` and `notepad_review_cursor` end with this step.
- A selection reported after the next key: Shift+Right and Right pressed in one batch from the start of "line 01". Without holding, Shift+Right was misjudged in 56 of 60 rounds and Right in 58 of 60, most rounds saying only "i"; with holding, 0 of 60 for either key, every round saying "l selected", "i", "l unselected". NVDA says only the character in most rounds, its Shift+Right script saying nothing while Right's waits; in one Notepad round, its own race, it said the words Verbatim says every time. Verbatim's words are each key's own effect, as it says every intermediate selection change (`docs/parity/text.md`, "TextInfo-equivalent layer"), a difference from NVDA's usual words (Dickson, 2026-10-26); `text_box_selection_keys` and `notepad_selection_keys` include this batch and declare the difference.
- Bursts of ten Down Arrows 30 ms apart: 0 misjudged in either build, since each answer came within the 30 ms before the next key, so these bursts never made a key wait behind another's read.
- A say-all caret move the application refuses: the console host is the one application found that refuses it (its cursor stays put when UI Automation selects a position). Right Arrow after say-all has moved Core's caret onto a wrapped command's second row was misjudged in neither build (0 of 30 each); with holding it cannot arise, since the read before each key replaces Core's caret. No scenario step covers it, since say-all in the console host reads the command's second row repeatedly (a separate fault, not traced), so its speech has no stable expectation.

## Limitations

- A stalled reducer thread delays caret keys in text fields, and every key pressed after one, until it recovers, where without holding keys reach the application at once whatever Verbatim is doing; the bound covers only the outpost's read. If Verbatim exits while keys are held, those keys are lost.
- The foreground changing between a key's press and its pass-on sends the key, and the keys deferred behind it, to the new foreground window, as NVDA's would; accepted (Dickson, 2026-10-22). If the new window is elevated, Windows drops them.
- A key auto-repeating is held press by press, so the application receives separate presses rather than repeats.
- Verbatim's own commands (a review command, select then copy) are not deferred: one pressed while a held key waits runs before the held key reaches the application.
- The path that passes a key on without its read, at `HELD_KEY_BOUND`, has been reached only in its unit test.
- A held key waiting behind another waits for that key's watch to be answered, at most 100 ms after that key was passed on, where without holding it reaches the application at once; a key that moves nothing, held with auto-repeat, therefore reaches the application about once every 110 ms ("A key held with auto-repeat").
- The lock keys pass at once while keys are held back, so a letter typed before Caps Lock and deferred behind a held key reaches the application after Caps Lock and is typed in the other case; a numpad key pressed behind a held key and before Num Lock likewise arrives as Windows translated it when it was pressed.
- The cost figures are from debug builds, as the measuring guide's key workloads are, and the text field traces are not comparable; a trace over a fixed number of keys would settle their cost.
