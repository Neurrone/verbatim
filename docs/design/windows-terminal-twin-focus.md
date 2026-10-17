# Windows Terminal's second terminal identity: a design for M5

Status: a design for M5, not implemented. Dickson decided (2026-10-18) that the rule belongs in a Windows Terminal app module, built when M5 brings extensions, rather than in the core focus judgement; the roadmap's M5 entry tracks it. It was reviewed adversarially on 2026-10-18 and the rule below includes that review's corrections. Until it is built, the tests accept the repeated "terminal" as an optional utterance (`docs/testing.md`, "Optional utterances"), which covers one repeat but not the late one described next, so that repeat can still fail a scenario.

Background is in [the focus pipeline](focus-pipeline.md), section 16.8 (the second identity, and NVDA receiving it) and section 18 (the listener's focus judgement). Evidence is in `target/e2e-evidence/github-38988118089/` (the GitHub run below), `target/e2e-evidence/github-39032739474/`, and `target/e2e-evidence/wt-twin-identity/` (the judge's runs `judge/baseline`, `judge/exp-one` and `judge/exp-full`, the scripts `identity_judge.py` and `suffix.py`, and the scratch scenario and experiments as `identity-judge-and-experiments.patch`).

## What still gets through

On GitHub run 38988118089, `windows_terminal_leave_flood` heard "window, terminal, blank, terminal, terminal, blank" for its second window. The listener's log for that window, in Windows Terminal's process, reads:

1. An element-selected event from `[14, 9213076, 0, 0, 0, 4, 10]`, 7 ms before the window's foreground event; dropped, with no window above it.
2. A focus from the usual identity, `[42, 983838, 4, 11]`, the terminal (`TermControl`, XAML) in island window 983838: compared unequal with the last focus (the first window's terminal), its keyboard focus read true, read and announced: the first "terminal".
3. Focus from the window itself, dropped because it did not have the keyboard focus; then the usual identity again, dropped as a repeat.
4. 2 ms after that repeat, three focus events from the second identity, `[14, 9213076, 0, 0, 0, 4, 11]`; the second was replaced by the third, and the third, read after the first, was dropped as its repeat. For the first, `CompareElements` with the current focus, the usual identity, answered false, so the duplicate rule did not apply. Its nearest window was 983838 and its plain keyboard focus read answered true, so it was read and announced: the second "terminal". The program's ancestor walk from it met the usual identity's parent, `[42, 983838, 4, 14]`.
5. 2 ms after the second identity was read, the usual identity again. `CompareElements` with the current focus, now the second identity, answered false; its keyboard focus read true; it was announced: the third "terminal".

Each "blank" is the line read with a terminal. The two extra announcements are the 42 to 14 to 42 flip. In local runs the second identity's plain keyboard focus read almost always answers false, which is why the flip is rare here and commoner on GitHub's slower runner.

## Rates

Measured with the judge below: 150 launches of Windows Terminal, each followed by a second window in the same process and then closed, against one running Verbatim, per registration:

- current main: the second identity was raised in 2 of 150 launches and 19 of 150 second windows; 1 launch announced the terminal twice (the second identity accepted first, then the usual one); no second window did.
- one handler object for the focus handler and every part of the group, as NVDA registers its one handler object: 2 of 150 launches and 11 of 150 second windows; 2 launches announced it twice.
- NVDA's whole global group on the root (its selective set: the ten further automation events, the three property changes, and active text position), with the listener's own handlers: none of 150 launches and 19 of 150 second windows; none announced it twice.

No registration prevents the second window's second identity: its rate is the same with NVDA's whole group, and NVDA receives it too (6 times in 15 pairs, focus pipeline section 16.8). At launch, the counts (0, 1 and 2 doubled in 150) are too few to tell the registrations apart. Both experiments were reverted. Verbatim does not cause the second identity. A rule is needed that recognizes it.

## What stays equal between the two identities

in all 156 focus events from a second identity in these runs and in the GitHub run, a focus from the usual identity in the same process arrived within 1 second, and the usual identity's runtime id after its `[42, window]` prefix (`[4, 11]`) equalled the last two parts of the second identity's. Every second identity's cached class name was `TermControl` and its framework XAML, as for the usual identity. When it had a window above it, its nearest window was the usual identity's island window, and its parent was the usual identity's parent. The second identity's second part (9213076 here) changes with every Windows Terminal process. Its last part is a counter shared by the process's elements, so two different elements never share it.

## The NVDA path

From the pinned source, `UIAHandler/__init__.py` 874 to 958: the focus handler drops an event from a hung window, records the sender as `lastFocusedUIAElement`, drops a sender that is not native UIA (nearest window), then applies the duplicate rule: when the last focus queued is a UIA object that does not allow duplicates, `compareElements` equates the sender with it, and that object's element reads `currentHasKeyboardFocus` true, the event is dropped. It then tests the nearest window with `shouldAcceptEvent`, makes the object (no window, no object), checks `shouldAllowUIAFocusEvent` (the sender's own keyboard focus, read live), and queues `gainFocus`. Nothing after that compares the new focus with the old: a UIA object's equality is `CompareElements` (`NVDAObjects/UIA/__init__.py` 1615), so NVDA would speak the second identity as a new focus whenever it passed `shouldAllowUIAFocusEvent`. In its captures it never did. The classification:

- Preserve: every step above, in its order, including the duplicate rule as it stands for every element that is not a twin.
- Replace: none.
- Omit: none.
- Deviation, Verbatim's own: the duplicate rule also drops a Windows Terminal terminal's second identity when the current focus is its usual identity, which `CompareElements` does not equate, as the rule below says. NVDA has no such rule, and would announce the flip the same way if it received the second identity with its keyboard focus reading true.

## The rule

The rule is described against today's listener judgement, `FocusReader::judge_before_reading` (focus pipeline section 18); M5 settles how a Windows Terminal app module reaches that step. Two identities are twins when all of these hold, every one taken from the event's cache or from a value the listener already has:

- one runtime id starts 14 and the other starts 42;
- the 42 id is longer than 2 parts, and its parts after `[42, window]` equal the same number of last parts of the 14 id;
- the two come from the same process;
- both have the cached class name `TermControl`, the same control type, and the XAML framework; the rule names Windows Terminal's terminal because it is the only element seen to do this;
- the sender's nearest window, found for the native check before the rule, equals the window the current focus was judged with.

The rule is one-directional. In step 3 of the judgement, after `CompareElements` has not equated the sender with the current focus:

- A sender with a 14 id that is the twin of a current focus with a 42 id is a repeat, and is dropped without reading the current focus's keyboard focus.
- A sender with a 42 id that is the twin of a current focus with a 14 id is not dropped. It is read and reported as a focus that replaces the current one, carrying the 14 id it replaces, so that the outpost reuses that focus's node rather than announcing a new focus, and finds and follows the 42 element.
- When `CompareElements` fails, the two are not twins, and the event is judged as today.

Why the 42 sender is not dropped: the outpost cannot find the 14 element by its path, so with a 14 focus it follows nothing. On GitHub run 38988118089 (`windows_terminal_leave_flood`, `outpost-spare-4.log` lines 191 to 219) the accepted 14 focus moved the focus-following subscription off the terminal ("the focus's element was not found; nothing is followed"), and only the 42 focus read after it put it back. A rule that dropped the 42 sender as a twin would leave the terminal unfollowed: no output, caret or text change from it would be heard.

Why the current focus's keyboard focus is not read for a twin: that read exists because `CompareElements` can equate different elements, and the twin test does not rely on it. In the baseline's doubled launch the second identity was accepted first and the usual one came 190 ms later; whether the second identity still answered that it had the focus then is not known. The sender's own keyboard focus is still read later (step 6) when the event is not a repeat. Focus that leaves the terminal and comes back is not affected: any focus in between, from UIA, MSAA, a foreground change or Core, supersedes the current focus, and the comparison is only ever with the current focus.

What it changes: the 14 event in step 4 of the GitHub run is dropped as a repeat of the usual identity, the current focus stays the usual identity, and the event in step 5 is dropped as a repeat by `CompareElements` as today. One "terminal" is said. For the doubled launch, where the second identity was accepted first, the usual identity's focus is read and reported as replacing it, so the outpost keeps the node, says nothing new, and follows the 42 element. Nothing else changes: a runtime id starting 14 that is not the twin of the current focus is judged as today.

Before implementing, collect evidence that the last part of the runtime id, the counter the twin test matches on, is never reused for a different element of the same process: split panes, move a pane, close and reopen panes, and open new tabs, and check that no two elements ever share it.

## Cost

Ledger entry ("A focus change, UIA, steady state" and the repeated focus, `docs/performance.md`):

- A focus that is not a twin: unchanged, the same calls in the same order. The twin test compares two runtime ids and four cached values in memory.
- A 14 twin of the current focus: dropped after the nearest window (`NormalizeElementBuildCache`, made today) and the comparison (`CompareElements`, made today), without the current focus's keyboard focus read: 2 calls, where a repeat that `CompareElements` equates costs 3 for a windowless sender. Today the same event costs those 2, the sender's keyboard focus read and the read itself (4 calls and the reads of the outpost and Core).
- A 42 twin of the current focus: the same calls as today's read; the outpost reuses the node instead of reporting a new focus.
- Memory: the last focus accepted (`Accepted`) keeps its runtime id, process id, class name, control type and framework, read from the event's cache when it was accepted, a few dozen bytes. No wake-ups and no work are added on any thread.
- The call-count ratchet gains one exact case in `call_counts.rs`: a 14 twin focus costs 2 calls.

Parity ledger: `docs/parity/focus-events.md`, "Duplicate focus suppression", would gain a bullet recording the twin rule as Verbatim's own, with this document as its record and its tests, and its status would stay "matched (verified)" with the deviation described in plain words. `docs/nvda/uia.md` needs nothing: NVDA's path is unchanged.

## The judge

- Unit tests of the twin test, a pure function of two cached descriptions: the pair from the GitHub run is twinned in both orders, with the direction telling drop from replace; two 42 ids, two 14 ids, a different process, a class name other than `TermControl`, a different framework, nearest window, or last parts, a failed comparison, and a 42 id of only `[42, window]` are not.
- A `mockapp` cross-process test: a scripted XAML-framework provider with a `TermControl` that answers under two runtime ids, the usual appended one and a raw id starting 14 with the same last parts, both answering that they have the keyboard focus. It raises focus from the usual identity, the second, and the usual again, and asserts exactly one focus reported to Core and that the 42 element is followed afterwards (a change it raises is heard); then the same with the second identity first. A second case puts two such controls, two panes, in one window, and asserts that focus moving between them is reported each time and never taken for a twin. On current main the first case reports three and two, which is the known failing case the test is validated against; it needs a mockapp command for a raw runtime id, which mockapp lacks today.
- The live rate judge (a scratch `measure_terminal_identities` scenario that opens Windows Terminal and a second window and closes both, repeated, and `identity_judge.py`, which splits the listener's log by foreground window and counts the second identity's focus events, those accepted, and terminal focuses accepted after the first in each window), validated on the GitHub run's log, where it counts the one accepted second identity and the two extra announcements, and on current main, where it counts the doubled launch. After the rule it must count no extra announcements over at least 150 launches with second windows, and the scenario must check the terminal is followed afterwards. It runs in about 11 minutes. It stays a scratch tool and is not committed, because it waits for fixed times; the patch is kept with the evidence.
