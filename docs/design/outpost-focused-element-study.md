# The outpost's read of its own focused element: a study

Status: study of 2026-10-29, decided by Dickson on 2026-10-29: keep `GetFocusedElementBuildCache` with the process-id check, and use it for difference 5 too, as [the focus pipeline rework](focus-pipeline-rework.md) records under "Decisions". The study's analysis below stays as the record of the options weighed. The experiment behind its figures is `experiments/the-outposts-focus-read-below-its-checked-window/`.

## In short

The question: can an outpost read its application's keyboard focus with no risk of connecting to another application, and what should the rework do about it?

The answer: not with `GetFocusedElementBuildCache`, which the outpost uses today; yes with a connection by handle to the focus window the outpost has checked as its application's, followed by a search below that window. The decisions this asks of Dickson, each with the recommendation:

- Close the race rather than state it as a limit: replace `GetFocusedElementBuildCache` in the outpost by the read below the checked window (defined below). Recommended, because the read runs at the moments another application's outpost is most likely connecting, and losing it costs a wrong report, a stall of up to five seconds, and the collision the rule of `docs/design/listener-without-connections.md` exists to prevent.
- How the search goes past one run's instruction limit: continue in further runs from where a run stopped, within the read's budget. Recommended over a `FindFirstBuildCache` fallback, which can enter another process's child window, and over giving up, which would lose the gap's reads in File Explorer's larger lists, where the gap is most needed.
- A plain read of the found element's `HasKeyboardFocus`, one call, as the listener reads a sender's: recommended, for the one recorded case in which a remote operation's keyboard focus read differs from a plain one.
- Difference 5 by a runtime-id search below the nearest window, never `GetFocusedElementBuildCache`: recommended.
- A measurement on real applications before the rework's step 3, comparing what `GetFocusedElementBuildCache` names with what the search finds, and counting the elements the search looks at: recommended, since mockapp cannot show either.

## Terms

- The focus read is the outpost's read of its application's focused control with no event element to start from: `focused_control_in` in `crates/verbatim-outpost/src/outpost/read.rs`.
- The checked window is the window with the keyboard focus as `GetGUIThreadInfo` names it, accepted by the focus read only when its provider process is the outpost's application (`served_by`, `crates/verbatim-uia/src/provider.rs`), it is not Core's hidden frame, and it is readable (not a ghost, not hung).
- The race is the focus moving to another application's window between that check and UIA's own reading of the focus inside `GetFocusedElementBuildCache`.
- The read below the checked window is the proposed replacement: `ElementFromHandleBuildCache` on the checked window, then the element with the keyboard focus found below it.

## The current exposure

### The read

`focused_control_in` reads the focus window, checks its provider process, its hidden-frame mark and its readability with local calls, decides its backend (`window_uses_uia`, the kept verdict or the raw probe), creates the worker's UIA client on first use, builds the node and ancestor cache requests, and only then, for a window judged UIA, calls `GetFocusedElementBuildCache` through the outpost's `FocusedElementReader` (`Uia::focused`, `crates/verbatim-uia/src/client.rs`). UIA reads the keyboard focus again itself inside that call and connects to whichever window has it then. Nothing after the call compares the element's process with the outpost's application: the proxy rule (`answered::checked` with window 0) refuses only an element served by UIA's own proxies.

For a window judged MSAA the read is already by handle: `verbatim_ia2::acquire::focused_snapshot` (`crates/verbatim-ia2/src/acquire.rs`) reads the focus window again, checks its owner, and reads that window's client object and its `accFocus`, NVDA's way. A window handle names one window of one process for that window's life, so this path cannot reach another application's window by a race; MSAA calls do not collide in any case (`e29`, `docs/uia-bugs.md`).

### Its callers today

- The focus-now answer (`read::focus_now`, called from the worker's `Query::FocusNow`, `crates/verbatim-outpost/src/outpost/worker.rs`). Core's app asks for it when an outpost becomes ready with a focus wanted (a new application, a replacement, a retirement), when the listener is replaced, when a menu or the task switcher ends (the fake focus), and on the shell's `FocusNow` command (`crates/verbatim-app/src/main.rs`).
- The read for focus-missing (`Worker::focus_missing` in `worker.rs`): when the listener's gap record sends focus-missing, one second after a `WinEvent` focus no UIA focus answered, or at once after the listener drops a focus for having no window above it, in a window the outpost judges UIA.

Nothing else in the product calls `GetFocusedElement`. The end-to-end agent calls it in two test-only reads (`crates/verbatim-agent/src/focus.rs`), which decision 10 of the listener design keeps.

### Uses the rework adds

- Step 3: the read at once after a listener's drop for not UIA or unknown in a window the outpost judges UIA.
- Step 5: the read on focus-missing, which replaces the gap read.
- Difference 22, step 14: the ask for the focus 200 ms after a return to the user's desktop, a focus-now answer.
- Difference 5, step 4: the builder's proposal of 2026-10-29, `GetFocusedElementBuildCache` with a comparison of runtime ids, to find a windowless text focus whose chain was cut before its nearest window.

### How wide the race is

In steady state the span is local work only: `GetGUIThreadInfo`, the provider-process check, `IsWindow`, `IsHungAppWindow`, the class name, and two cache requests built. Measured in mockapp in a debug build (the build the end-to-end suite runs), it took 65 and 69 microseconds at the median in two runs of 1,000, 122 and 419 at the 99th percentile, and 763 at worst. It is wider on the first read of a window: the raw probe runs inside it, 0.5 ms at the median, 3.6 ms at the 95th percentile and 211 ms at worst over 53 first sights (`docs/design/listener-without-connections.md`, section 8), bounded by `RAW_PROBE_TIMEOUT_MS`, 3 seconds; and the worker's UIA client is created on its first use. The first read of a window is the focus-now answer of a new outpost, whose application has just started.

The reads run when the focus is likely to be moving: just after a menu or the switcher ends, just after an application starts, and in the gap, which exists because a new application's focus events arrive late or in bursts. The race has not been observed; nothing logs it.

### What losing it costs

When the focus has moved to application B's window by the time UIA reads it:

- The outpost of application A makes a new connection into B, which the rule of section 2 of `docs/design/listener-without-connections.md` reserves to B's own outpost. B's outpost is likely connecting at that moment, since B has just taken the focus: its own lookup by handle to find the element the listener read (`focus_path.rs`), while the listener's held calls into B are pending. Those are the conditions of the second form of the collision in `docs/uia-bugs.md`'s first entry, two processes' new connections waiting in B and a third process's call pending, under which a new connection fails and returns UIA's window proxy. The failing connection can be B's outpost's, and the proxy rule then gives B no answer.
- If B is hung, A's worker waits on B (E5 in `docs/design/focus-pipeline.md`, section 11): up to `CONNECTION_TIMEOUT`, 2 seconds, in the connection, and `TRANSACTION_TIMEOUT`, 5 seconds, if B stops answering after it, within the worker's 10-second `HANDLING_DEADLINE`. A's own events wait behind it.
- If B answers, A reports B's element as its application's focus, with the checked window's facts, and holds it. A then sends its enrichment program into B, a remote operation from a process other than B's outpost (the first form's trigger for every new connection B's outpost makes meanwhile), and moves its focus-following subscription onto B's element, a registration into B. That B's focus would then be spoken twice, once from each outpost, or once wrongly placed, is inferred from the code, not observed.

## NVDA

NVDA reads the focus with no event the same way as the outpost does today, from the pinned source:

- `objectWithFocus` (`NVDAObjects/__init__.py` 351 to 366) finds the best API class for the relation "focus": the window is the foreground window's thread's `hwndFocus` (`NVDAObjects/window/__init__.py` 163 to 179), whose class lists and `isUIAWindow` choose UIA or IAccessible (84 to 118, `isUIAWindow` at 112).
- For UIA it then calls `getFocusedElementBuildCache` (`NVDAObjects/UIA/__init__.py` 1531 to 1552) and ignores a non-native element, `isNativeUIAElement` comparing the element's cached process id with NVDA's own and judging its nearest window (`UIAHandler/__init__.py` 1516 to 1537). So NVDA has the same check-then-read order and the same race, and checks afterwards only that the element is not its own. Being one process, NVDA's own calls cannot collide with each other (`docs/uia-bugs.md`, the first entry's NVDA section). Only the probe runs through `watchdog.cancellableExecute` (`UIAHandler/__init__.py` 1275 to 1285); the focused-element call is a plain call.
- For IAccessible it reads the window's client object and loops on `accFocus` (`NVDAObjects/IAccessible/__init__.py` 542 to 563), by handle, as the outpost's MSAA path does.
- `objectWithFocus` is what `_handleUserDesktop` queues as a `gainFocus` 200 ms after a return to the user's desktop (`IAccessibleHandler/__init__.py` 796 and 809 to 813), what `_fakeFocus` reads after a menu ends (991 to 1002), and what `_setInitialFocus` reads at start (`core.py` 425 to 435).
- NVDA has no read for a focus it cannot attribute: a `WinEvent` focus in a native UIA window is dropped (`IAccessibleHandler/__init__.py` 576 to 582, in `winEventToNVDAEvent`), and a focus UIA never delivers is lost (decision 11 of the listener design is Verbatim's departure).

The read below the checked window is therefore a deliberate difference of mechanism with the same intended answer, the element with the keyboard focus in the focus window. It answers differently from NVDA only where a provider's `GetFocus` and its elements' `HasKeyboardFocus` disagree, or where the focus is outside the window's own tree (below, "What each option cannot find").

## The options

### Today's call, stated as a limit

One call, one connection; the provider answers with `GetFocus` from the fragment root down, a few calls for any tree (inferred from Microsoft's provider documentation, not measured). It finds whatever the system's focus is, in any window. Its cost when the race is lost is the list above, and the rework adds four more uses. Stating it as a limit is defensible on frequency alone, since the steady-state span is tens of microseconds, but the race is lost at exactly the moments it is wide or the focus is moving, and its consequences break the rule the listener design was built on.

### Today's call, checked after it

Reading the element's cached process id (no call) and refusing an element of another process removes the wrong report and everything that follows it: the enrichment program into B and the subscription. It cannot undo the connection already made into B, so the collision and the stall on a hung B remain. Worth having only if the call stays.

### A connection by handle to the checked window, then the focus below it

`ElementFromHandleBuildCache` on the checked window is a connection by handle to a window whose provider process the outpost has just verified, and a handle cannot come to name another process's window, so the connection reaches the outpost's own application whatever the focus does meanwhile. It is one connection, as today's call is. The element with the keyboard focus is then found below it in one of three ways.

- The window's own element. When the cached `HasKeyboardFocus` of the window's element is true, it is the focus, with no further call: a Win32 control with a provider of its own, such as Notepad's text area, is typically its own focus window's element (not measured here). It cannot find a focus below the window's element, which is every other case.
- A remote program searching the window's raw-view descendants, depth first, for the first element whose `HasKeyboardFocus` is true, filled as a node, as the controlled selection's program searches for a runtime id (`crates/verbatim-uia-rops/src/controlled.rs`). Not entering an element that is a window of its own keeps the search inside the checked window, since the focus window is the deepest window holding the focus. One round trip; about 21 instructions per element looked at, 28 with that pruning, so one run reaches about 470 elements, or 355 pruned, before UIA's limit of 10,000 executed instructions (`crates/mockapp/tests/instruction_limit.rs`), and about half that under the margin `docs/performance.md` asks of a program's worst case. Worst case: a focus after more elements than a run reaches, as on a late row of a file list in details view (120 rows of four cells stopped the run in mockapp), which needs further runs from where the last stopped, each importing the element it ended on. Against a provider that stops answering, each run is bounded as a whole by the transaction timeout (`docs/crates/verbatim-uia-rops.md`, "A stalled or exited provider").
- One `FindFirstBuildCache` with a `HasKeyboardFocus` condition over the descendants. One call and no instruction limit: it found the focus in every mockapp case, the 600-element grid included. It cannot prune by window, so it can step into a child window of another process, which UIA's client side connects to; and against a provider that stops answering it waits a transaction timeout for each provider call it makes (`docs/uia-bugs.md`, "A timeout bounds each provider call, not an operation": 13.2 seconds under a 1-second timeout), so it needs a deadline of its own on an abandonable thread (`crates/verbatim-uia/src/bounded.rs`).

Each search makes about 16 provider calls per element it looks at in mockapp, most of them `ProviderOptions`, against the few of `GetFocus`; the remote search and `FindFirstBuildCache` cost the same there (1,764 and 1,595 calls for a 100-item list). All of it is off the event path: none of these reads is made for a UIA focus event, which the listener reads from its sender.

### What each option cannot find

- `GetFocusedElementBuildCache` finds the system's focus wherever it is, which is both its use and its fault; it can also name an element other than the event's sender, as Start's search results raise their focus while it stays on the search box (018e62d5).
- The searches find only an element the provider marks with `HasKeyboardFocus` inside the window's raw-view tree. They give no focus where a provider's `GetFocus` names an element outside that tree or marks none; they stop at the first element in tree order that answers true, so a provider that marks two would give the first; and inside a remote operation an element can answer true where a plain read answers false (`docs/uia-bugs.md`, "A remote operation's keyboard focus read can differ from a plain read", Windows Terminal's second identity), which the plain read of the answer catches, at one call. How often real providers fall into these is unknown, hence the measurement recommended below.

### Difference 5

The listener's read whose walk was cut leaves the outpost the focus's runtime id, the chain it did read, and the nearest window, which every read finds; the path walk down from that window finds the focus only when the window's element is its parent. A remote search below the nearest window for a runtime id needs nothing about where the focus is now, so it has no race and finds the event's element even after the focus has moved on, which is the element whose text the outpost reads. Searching for the outermost element the cut chain reached, then walking down the chain's known path below it (`element_by_path`), looks at no more elements than searching for the focus itself, since a depth-first walk reaches an ancestor before its descendants. About 26 instructions per element, about 385 elements a run. `GetFocusedElementBuildCache` with a runtime-id comparison, the builder's proposal, has the race and also fails whenever the focus has moved on, which is common on the slow focus difference 5 is about.

## The measurement

In mockapp (`experiments/the-outposts-focus-read-below-its-checked-window/`): every search that answered found the focused node, by handle and below the window, in the fixture's list item whose parent has no window, the item 60 groups deep, and the last item of lists of 100, 150, 250 and 400; the instruction and provider-call figures are those above. It cannot show what real providers do: on a test's own desktop nothing has the system focus, so `GetFocusedElement` answers the desktop, and mockapp answers `GetFocus` and `HasKeyboardFocus` from one state. Two questions remain for real applications, both answerable from a scratch build in a batch of the scenarios the rework already runs: whether the search finds the element `GetFocusedElementBuildCache` names in every read, and how many elements lie before the focus in File Explorer's folder windows, Settings and Windows Terminal.

## Recommendation

Adopt the read below the checked window for every focus read and for difference 5, with continuation runs, the window-pruned search, and the plain read of the answer. Build it in step 3, the first step that adds a use, so steps 3 and 5 and difference 22 inherit it, and reuse its search in step 4 for difference 5. Measure on real applications first, as a batch of its own or within step 1's runs if they have not yet been made. No step undoes recently finished work: the listener read fixes of 0cd180ab are untouched, and difference 5's fix was stopped before it was built.

The exact changes proposed to `docs/design/focus-pipeline-rework.md`, not applied:

1. A new subsection after "A window's name, and the one fallback", headed "The outpost's read of its own focus":

   "Every read the outpost makes of its application's focus with no event element to start from (the focus-now answer, the read after a listener's drop in a window it judges UIA, focus-missing's read, and the ask after a return to the user's desktop) starts from the window with the keyboard focus that it has checked as its application's, never from `GetFocusedElementBuildCache`, which reads wherever the focus is when it runs and so can connect to another application after the check (`docs/design/outpost-focused-element-study.md`). In a window it judges UIA it connects by handle (`ElementFromHandleBuildCache`), and the window's element is the focus when its cached `HasKeyboardFocus` is true; otherwise one remote program walks the window's raw-view descendants depth first, entering no element that is a window of its own, to the first whose `HasKeyboardFocus` is true, filled as a node, and a run stopped by the instruction limit is continued by another from the element it ended on, within the read's budget; the element found is read plainly for its keyboard focus, as the listener reads a sender's, and nothing is reported when it has none. In a window it judges MSAA it reads the window's client object and `accFocus`, as today and as NVDA does. NVDA reads the focus with `GetFocusedElementBuildCache` (`NVDAObjects/UIA/__init__.py` 1531 to 1552): a deliberate difference of mechanism, which answers differently only where a provider's `GetFocus` names an element its `HasKeyboardFocus` does not mark in the window's own tree. Costs: one connection, as before; no further call when the window is the focus, otherwise one round trip per about 350 elements before the focus and one plain read; about 16 provider calls per element looked at; nothing on the event path."

2. Difference 5, its fix replaced by: "the route starts from the nearest window the read found, which every read has (`focus_path.rs`); when the read's chain stops below that window, the outermost element it reached is found below the window by the focus read's search, by its runtime id instead of the keyboard focus, and the chain below it by its path as now; never `GetFocusedElementBuildCache`." The "Heard" sentence stays.

3. Step 3 of the order of work, its clause on the read after a drop becoming: "the outpost reading its application's focus at once after a listener's drop in a window it judges UIA, by the read of its own focus below the checked window ("The outpost's read of its own focus"), which this step builds and which replaces `GetFocusedElementBuildCache` for the focus-now answer and the gap read too, stamped with the dropped event's observation, ..." with the rest of the step as written.

4. Step 4, its last clause becoming: "and the windowless text focus's route from the nearest window the read found, with the runtime-id search below it when the read's chain stops short of that window."

5. Step 5, "focus-missing, and the outpost's read on it" becoming "focus-missing, and the outpost's read on it below the checked window".

6. "Edge cases, limitations and what must be measured", a new item: "The outpost's read of its own focus finds only an element its provider marks with `HasKeyboardFocus` inside the focus window's raw-view tree; where `GetFocus` names another, nothing is reported for that read, a stated limit. Measured before step 3, in a scratch build over `explorer_folder_window`, `settings_system_page`, `windows_terminal_two_windows` and `measure_focus_explorer`: for each focus read, the element `GetFocusedElementBuildCache` names against the search's answer, and the elements the search looked at. Any disagreement, or a read needing more than three runs, goes to Dickson before step 3."

7. "Mechanisms removed and added", added: "the outpost's read of its own focus below the checked window, its search program and continuation"; removed: "`GetFocusedElementBuildCache` and the outpost's `FocusedElementReader`".

8. "Tests", step 3, added: mockapp tests `the_focus_read_finds_the_focus_below_its_checked_window`, `a_focus_past_one_runs_instruction_limit_is_found_by_the_next_run` and `the_focus_read_reports_nothing_when_the_found_element_has_no_keyboard_focus`, and the search's worst case pinned in `instruction_limit.rs`; updated: `call_counts`' focus-now counts, and mockapp's stand-in for `GetFocusedElementBuildCache` (`crates/mockapp/tests/common/outpost.rs`) removed, since a connection by handle can be made to mockapp on a test's own desktop. Step 4's `a_slow_windowless_text_focus_is_found_from_its_nearest_window` takes a focus below a windowless parent with a cut chain.

9. Optionally, "Why no smaller design", after "relies on `GetFocusedElementBuildCache` and a comparison of runtime ids": "which also reads wherever the focus is when it runs, so it can connect to another application (`docs/design/outpost-focused-element-study.md`)".

## Limits of this study

- Nothing was measured on a real application: the interactive desktop was in use. The race's frequency, real trees' sizes and real providers' agreement between `GetFocus` and `HasKeyboardFocus` are open.
- The internal steps of `GetFocusedElementBuildCache` (a read of the focus window, then the fragment root's `GetFocus`) are taken from Microsoft's provider documentation, not measured.
- Wall-clock times in the experiment were taken with seven cases in parallel and are not cost figures; provider calls and instructions are.
- Pre-existing, outside this study's scope: the MSAA path reads the focus window a second time, so the window whose facts the answer carries can differ from the one the node was read in; harmless for connections, since both are checked against the application.
