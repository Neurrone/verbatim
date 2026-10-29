# The focus pipeline against NVDA: the audit of 2026-10-29

Status: input to the revised design of the focus pipeline rework (`docs/roadmap.md`, M4, "The focus pipeline rework", scope widened by Dickson on 2026-10-29). Read-only audit of Verbatim at 33ee5255 against the pinned NVDA (732d1ce2), in three parts: the UIA focus path, the MSAA and `WinEvent` path, and what follows acceptance (the focus's ancestry, the foreground and the speech). Each part compared, at every step, the rule, the order, what happens when a call is slow, fails or finds its element gone, and what is spoken. Every NVDA claim below was checked against the pinned source; file references are to `nvda/source/` and `crates/`, as of the audited commits.

Terms: the listener is the process that reads every UIA focus; an outpost is the process serving one application; Core is the reducer; "ancestors unknown" is a focus reported without its chain, after which Core keeps the previous focus's chain.

## What the audit found, in one paragraph

Twenty-five differences no decision of record covers, after merging those two parts found from either end; three recorded but never decided; about forty deliberate; and eleven open questions that only a capture or an experiment settles. The agreed rework fixes two of the twenty-five. Most of the rest share one cause: where NVDA meets a slow or failed call by falling back and carrying on (a failed parent read continues through the window hierarchy; a cancelled read degrades to an empty value), Verbatim gives up, by marking the ancestors unknown, dropping the focus, or reporting a partial chain as whole. The earlier audits compared rules one by one and so never saw this class, because NVDA has no rule there, only its fallback.

## Differences no decision covers

### The focus's ancestry and the window

1. A new window whose focus is read with its ancestors unknown is announced one focus late, or never. Core enters no container and keeps the previous chain (`reduce.rs` 898 to 906, 2387 to 2389), so the window is spoken at the next focus, and never when the window left had the same title (a second "Windows PowerShell" console). NVDA blocks, and its watchdog's cancel and the window-hierarchy fallback still give the window (`NVDAObjects/UIA/__init__.py` 2136 to 2145; `eventHandler.py` 402 to 419). The budget and the kept chain are deliberate (Dickson, 2026-10-20 and 2026-10-21); this consequence across windows was not weighed, since the 2026-10-21 decision rested on a focus in the same window. The Windows Terminal launch case. Rework: not changed.
2. A partial chain is reported as whole. A failed or empty MSAA `accParent` ends the walk as complete (`verbatim-ia2/src/acquire.rs` 742 to 744; `outpost/read.rs` 692 to 707), where NVDA falls back to the window parent (`NVDAObjects/IAccessible/__init__.py` 1242 to 1261), and the UIA walks report the same failure as unknown since 2026-10-21. So a dialog is left out, then said again at the next Tab. The same holds for the continuation across the process boundary when the boundary window cannot be acquired (`read.rs` 375 to 388), and for a chain cut at the 64-ancestor limit (`verbatim-uia-rops/src/focus.rs` 963 to 966), where NVDA walks 100 and then jumps to the desktop (`api.py` 112 to 126).
3. Reused ancestors are stale. The listener reuses its last read of the application above the meeting point however old (`focus_read.rs` 882 to 897, 1116 to 1133), and the outpost splices its own last chain (`read.rs` 67 to 72, 365 to 368). NVDA meets only its own previous focus line, which after a visit to another application means it reads the whole chain again live (`api.py` 111 to 147). Heard: after Alt+Tab back to Windows Terminal whose title a program changed, the old title.
4. A focus on a list, tab control or combo box loses its whole read when its selected item is gone or its selection read times out (`verbatim-uia-rops/src/focus.rs` 301 to 334; `verbatim-uia/src/client.rs` 930 to 962): the focus is reported from the event's cache with ancestors unknown. NVDA's walk does not read the selection.
5. A windowless text focus read with its ancestors unknown cannot be found by the outpost, so its text, caret and terminal output are not read until the next focus (`outpost/focus_path.rs` 53 to 90; `worker.rs` 3705 to 3766): the route starts from an ancestor with a window, and the nearest window the read found is not used. Windows Terminal's terminal and XAML text boxes when slow.
6. Two different MSAA containers with the same role and name in one window count as one (`focus_rules.rs` 317 to 325), where NVDA's equality also needs the window handle, identity and location (`NVDAObjects/IAccessible/__init__.py` 906 to 959). A second unnamed toolbar or list is not announced.
7. Invisible MSAA ancestors and the border and white space roles are announced as containers (`reduce.rs` 2434 to 2460); NVDA treats them as layout (`NVDAObjects/__init__.py` 910 to 945).
8. NVDA's `SensitiveSlider` rule is not ported (`NVDAObjects/UIA/__init__.py` 2708 to 2712): NVDA says the slider with its value when the focus enters its thumb.
9. A UIA dialog's own text is given up after one second (`outpost/read.rs` 770 to 775); NVDA waits, so a slow message box loses its question.

### Judging and ordering focuses

10. A focus returning to an element while Core's notice is in flight is dropped as a repeat (`focus_read.rs` 1301 to 1323). Rework: fixed.
11. The listener and the outpost decide one window's backend differently, so a focus comes a second late through the gap (`worker.rs` 3511 to 3524, 3666 to 3671). Rework: fixed.
12. Core judges an accepted focus against the foreground a second time, from window facts read when the outpost publishes it (`reduce.rs` 296 to 310), and `report_foreground` tests a foreground change again after its read (`worker.rs` 3464 to 3473); NVDA tests once, before queuing. A slow focus published after Alt+Tab is never spoken. D14 as amended on 2026-10-05 makes the second test; no record weighs this loss.
13. The outpost's intake tests a listener-read focus for a hung window a second time (`outpost/intake.rs` 1438 to 1445, 1495 to 1506): a focus raised just before a stall of more than five seconds is dropped, where NVDA speaks it when the application resumes (the capture of 2026-10-17).
14. A timed-out nearest-window lookup is taken as a gone sender (`verbatim-uia/src/nearest.rs` 92 to 107), so an open gap makes the outpost ask the busy application at once. NVDA drops the focus (`UIAHandler/__init__.py` 1437 to 1444), and Verbatim's own `GAVE_UP` reason says not to read into an application that just failed to answer.
15. The listener's provider probe gives up at three seconds and takes the window as not UIA (`arbitration.rs` 194 to 206). NVDA's probe on the UIA path runs on its event thread and is never cancelled (`watchdog.py` 417 to 420); `docs/parity/backends.md` describes NVDA's cancelled probe, which happens only on its main thread.
16. A UIA menu opening is dropped behind any focus of the application waiting to be read or being read (`focus_read.rs` 1925 to 1942), where NVDA counts only focuses it accepted and queued (`UIAHandler/__init__.py` 772 to 777; `eventHandler.py` 48 to 107).
17. Verbatim does not wait out NVDA's 10 ms delayed pump after a non-focus event (`IAccessibleHandler/internalWinEventHandler.py` 192 to 193; `core.py` 1004 to 1013), so a popup start or foreground change and the focus a millisecond behind it are handled apart (`outpost/intake.rs` 140 to 147). A keyboard-opened context menu may say "context menu" before its item where NVDA says only the item; a capture settles the speech.
18. The focus read after a menu or the switcher closes has no focused-state or duplicate test (`verbatim-core/src/shell.rs` 170 to 188; `outpost/read.rs` 919 to 1004), where NVDA's `_fakeFocus` applies both (`IAccessibleHandler/__init__.py` 991 to 1003, 762 to 783).
19. The MSAA focused-state test stops at a failed `accParent` and never crosses into a UIA parent (`verbatim-ia2/src/acquire.rs` 455 to 484); NVDA's walks on through the window fallback (`NVDAObjects/IAccessible/__init__.py` 874 to 892).
20. A duplicate focus's newer reading moves the baseline for change speech (`reduce.rs` 414 to 433, 1333 to 1335), so a name change a repeat carried in first is never spoken; NVDA compares with what it last spoke (`speech/speech.py` 768 to 781). Rework: resending kept facts moves the baseline to older facts instead; its step 2 must cover this.
21. Speech is cut on any change of top-level window (`reduce.rs` 1076 to 1090), where NVDA cuts only when the chain diverges just below the desktop (`eventHandler.py` 402 to 413), so not for a combo box's drop-down whose parent is the combo box. A probe of the drop-down's MSAA parent confirms it.

### Events not taken

22. Returning from the secure desktop: NVDA speaks the focus again 200 ms after a UAC prompt, Control+Alt+Delete or the sign-in screen (`IAccessibleHandler/__init__.py` 786 to 813); Verbatim hooks no desktop switch and says nothing.
23. The virtual desktop's name ("Desktop 2" after Windows+Control+Right) is never spoken (`eventHandler.py` 338 to 375, 414, 529 to 531).
24. File Explorer's redundant MSAA focus on its search band is not refused (`appModules/explorer.py` 48 to 51, 235 to 241), so "pane" may follow "Search edit"; a hand run on Windows 11 24H2 confirms it occurs.
25. An application's events are lost between its outpost's death and `HookRemoved` reaching the listener (`listener.rs` 712 to 716; `supervisor/owner.rs` 1319 to 1328): a context menu opened then is silent.

## Recorded but never decided

- The foreground object is the last foreground report's window, or the top of the focus's chain, not the real foreground window NVDA takes (`reduce.rs` 974 to 976, 1025 to 1050; `eventHandler.py` 403 to 411).
- Everything handed to the synthesizer is stopped when any of it has expired (`verbatim-speech/src/manager.rs` 995 to 1004), where NVDA stops up to the newest expired utterance; an audio constraint, stated.
- The menu-item clause of focus speech culling is judged only for the focus just left (`reduce.rs` 1169 to 1181), where NVDA tests every waiting utterance.

## Open questions, each with what settles it

- What NVDA says when its ancestry walk's UIA call is cancelled: whether `CoCancelCall` reaches a UIA client call at all, and whether the fallback's unguarded `ElementFromHandleBuildCache` (`NVDAObjects/UIA/__init__.py` 1554 to 1557) ends the event. A capture of a mockapp provider stalling only its parent navigation for 15 seconds, the focus arriving from another application and from within.
- Whether `CoCancelCall` ends a UIA client call made by Verbatim: an experiment as `experiments/cocancelcall-ends-an-msaa-com-call-but-not-a-window-message/` did for MSAA. Handled apart from the rework, in M6.1 (Dickson, 2026-10-29; `docs/roadmap.md`), since cancelling frees a thread but changes nothing spoken.
- A stall after an MSAA focus is judged: the ledger says NVDA drops the focus, but its getters catch the cancel and degrade, so NVDA would speak a degraded focus late; Verbatim abandons at 10 seconds. A mockapp `stall` on its first `accParent`, 15 seconds, captured under NVDA.
- NVDA's object creation for a gone UIA element: which roles' `findOverlayClasses` raise and drop the focus. A capture of a document element destroyed while a slow event holds NVDA's event thread.
- How long `UiaHasServerSideProvider` waits on a window that does not pump, which sizes difference 15: timed against mockapp's `stall` at 3, 10 and 30 seconds.
- A repeated MSAA focus on a simple child: NVDA speaks it again, Core's duplicate rule drops it. A capture of Home on the first item of a `SysListView32` and a `ListBox`.
- The focus-now read onto a hung window: NVDA speaks a plain window object by its text, Verbatim nothing. A capture of Alt+Tab to mockapp stalled for 30 seconds.
- A nameless foreground window: NVDA's source suggests "pane", the ledger says silence matches. A capture of mockapp bringing a nameless window to the front with no focus event.
- Whether UIA's raw view puts a same-process owned window under its owner (difference 21): a raw-view walk from a WPF owned dialog.
- File Explorer's `Start` window: whether any keyboard route on Windows 11 24H2 puts the MSAA focus there, which decides porting NVDA's `StartButton`.
- What the outposts' foreground tests compare with once `CoreFocus` is gone: a decision the rework needs before its repeats step.

## Deliberate differences

Each has its record: one reading thread per application (D9, D13); the 2-second read budget and the kept chain (listener design section 5; `docs/parity/focus-events.md`); the 5-second transaction timeout; the moved-on rule and the 11-second `READ_DEADLINE`; the walk stopping at the process boundary and the MSAA continuation; the lifetime UIA verdict and the raw probe (D15); the gap and the immediate read for a sender with no window (decision 11); repeats and foreground tests against Core's focus (D14, changed by the rework); the stale drop across outposts; the marker; the hidden frame; console routing; application modules' window rules and File Explorer's rules, scheduled with M5; the widened child redirect; the menu slot and run rules; the fake focus's skip rule; the selected item spoken with a focused list; the focus and its line as two utterances; the silent re-read after an outpost is replaced; the 2-second MSAA ancestor budget; JAB, IA2, elevated windows and the secure desktop not yet built. The "not responding" earcon has no parity entry and is probably deliberate.

## Records to correct

- `docs/parity/backends.md`: NVDA's timed-out probe as "no provider" holds only on NVDA's main thread; the focused-state test does not match "ancestors included".
- `docs/design/listener-without-connections.md`: an application answering in 2 to 20 seconds is not always "spoken earlier and with less"; a windowless focus with no kept verdict is not spoken at all.
- `docs/parity/focus-events.md`: "A slow read the user has moved on from" rests on a claim NVDA's source contradicts for MSAA; "When speech is cut off" states NVDA's rule as any new top-level window.
- `docs/parity/focus-context.md`: the shell's foreground filter is ported (since 13dcd2db); NVDA's MSAA equality weighs role and name only after window, identity and location; the role exclusions for presentability are not all carried.
- `docs/parity/backends.md` and the listener design: the MSAA and listener budgets leave a new window unannounced until the next focus, which neither states.
