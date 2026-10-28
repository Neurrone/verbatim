# Windows Terminal's second terminal identity: a design for M5

Status: a design for M5, not implemented. Dickson decided (2026-10-18) that the rule belongs in a Windows Terminal app module, built when M5 brings extensions, rather than in the core focus judgement; the roadmap's M5 entry tracks it, and `docs/design/extensions.md`, section 6, places the module's hook. Until it is built, the tests accept the repeated "terminal" as an optional utterance (`docs/testing.md`, "Optional utterances"), which covers one repeat but not a late one, so that repeat can still fail a scenario.

Background is in [the focus pipeline](focus-pipeline.md), section 16.8 (the second identity, and NVDA receiving it) and section 18 (the listener's focus judgement). Evidence is in [`experiments/windows-terminal-raises-focus-from-a-second-runtime-identity/`](../../experiments/windows-terminal-raises-focus-from-a-second-runtime-identity/readme.md).

## The problem

Windows Terminal sometimes raises its terminal's focus from a second identity, a runtime id starting 14 and carrying a counter, besides the usual one starting 42 with its window's handle. `CompareElements` says the two differ, so the listener announces each as a new focus, and "terminal" is said twice, or three times when the focus flips from the usual identity to the second and back. Locally the second identity's plain keyboard focus read almost always answers false, so the flip is rare there and commoner on GitHub's slower runner.

## Rates

Over 150 launches of Windows Terminal, each followed by a second window in the same process, the second identity was raised in 2 launches and 19 second windows with the listener's registration, and in no launch but still 19 second windows with NVDA's whole global group on the root. No registration prevents the second window's second identity, and NVDA receives it too (focus pipeline section 16.8). Verbatim does not cause it, so a rule is needed that recognizes it.

## What stays equal between the two identities

In every focus event from a second identity measured, a focus from the usual identity in the same process arrived within 1 second, and the usual identity's runtime id after its `[42, window]` prefix equalled the same number of last parts of the second identity's. Both had the cached class name `TermControl` and the XAML framework, the same nearest window when the second had one, and the same parent. The second identity's second part changes with every Windows Terminal process; its last part is a counter shared by the process's elements.

## The NVDA path

NVDA's focus handler drops a sender that `compareElements` equates with the last focus while that focus reads that it has the keyboard focus, and otherwise speaks the sender as a new focus if its own keyboard focus reads true; it would therefore announce the second identity the same way, though in its captures it never did. Every step of that path is preserved. The deviation is Verbatim's own: the duplicate rule also drops a Windows Terminal terminal's second identity when the current focus is its usual identity.

## The rule

Two identities are twins when all of these hold, each taken from the event's cache or a value the listener already has: one runtime id starts 14 and the other 42; the 42 id is longer than 2 parts and its parts after `[42, window]` equal the same number of last parts of the 14 id; the two come from the same process; both have the cached class name `TermControl`, the same control type and the XAML framework, since Windows Terminal's terminal is the only element seen to do this; and the sender's nearest window equals the window the current focus was judged with.

The rule is one-directional, and applies only after `CompareElements` has not equated the sender with the current focus (in today's judgement, step 3 of `FocusReader::judge_before_reading`, focus pipeline section 18):

- A 14 sender that is the twin of a current 42 focus is a repeat, dropped without reading the current focus's keyboard focus.
- A 42 sender that is the twin of a current 14 focus is read and reported as replacing it, carrying the 14 id, so the outpost reuses that focus's node, announces nothing new, and follows the 42 element. Reason: the outpost cannot find the 14 element by its path, so with a 14 focus it follows nothing, and dropping the 42 sender would leave the terminal's output, caret and text changes unheard.
- Any other event, a 14 sender that is no twin of the current focus included, is judged as today.

The current focus's keyboard focus is not read for a twin because that read exists only because `CompareElements` can equate different elements, which the twin test does not rely on; the sender's own keyboard focus is still read when the event is not a repeat.

Before implementing, collect evidence that the runtime id's last part is never reused for a different element of the same process: split panes, move a pane, close and reopen panes, and open new tabs.

## Cost

A focus that is not a twin makes the same calls in the same order; the twin test compares two runtime ids and four cached values in memory. A 14 twin is dropped after 2 calls (the nearest window and `CompareElements`), where today it costs 4 and the reads of the outpost and Core; a 42 twin makes today's calls. The last accepted focus keeps its runtime id, process id, class name, control type and framework, a few dozen bytes, and no wake-ups are added. The call-count ratchet in `call_counts.rs` gains the case of a 14 twin costing 2 calls. `docs/parity/focus-events.md`, "Duplicate focus suppression", gains a bullet recording the rule as Verbatim's own, its status staying "matched (verified)".

## The judge

- Unit tests of the twin test: the measured pair twinned in both orders, with the direction telling drop from replace; not twins for two 42 ids, two 14 ids, a different process, class name, framework, nearest window or last parts, a failed comparison, and a 42 id of only `[42, window]`.
- A `mockapp` cross-process test, needing a mockapp command for a raw runtime id: a scripted XAML `TermControl` answering under both identities raises focus from the usual, the second and the usual again, and then with the second first, asserting exactly one focus reported to Core and that the 42 element is followed afterwards; a second case with two such panes in one window asserts that focus moving between them is reported each time.
- The live rate judge in the experiments folder must count no extra announcements over at least 150 launches with second windows, with the terminal followed afterwards. It stays a scratch tool, not committed, because it waits for fixed times.
