# Experiment: accFocus names no other process's child window

This experiment serves `docs/design/cross-process-calls-study.md`. The outpost's MSAA read of its application's focus, `verbatim_ia2::acquire::focused_snapshot` (`crates/verbatim-ia2/src/acquire.rs`, lines 1204 to 1220), reads the focus window with `GetGUIThreadInfo`, checks that its owner is the outpost's application, acquires that window's client object, and follows its `accFocus`, keying the result by the window `WindowFromAccessibleObject` gives it. The review of 2026-10-29 asked whether `accFocus` can name another process's object, so that the read follows it into that process. This experiment forces the case in which that looks most likely: the keyboard focus moves, after the check, into a child window of the checked window that another process serves.

## Setup

The program is `xproc.rs` in `experiments/cross-process-calls-study-program/`, with `XP_EXP=acc_focus`, on the interactive desktop.

- P: mockapp on `boundary_win32_host.json`, `win32` backend, shown: a plain Win32 window, whose client object is oleacc's standard proxy, with a child window of its own on its thread ("Embedded").
- C: mockapp on `small.json`, MSAA backend, made P's child with `SetParent` and shown.
- The test thread attaches its input to P's thread and C's thread, so it can set the keyboard focus in either.

Each trial brings P to the front and focuses P's own window, then makes the check (`GetGUIThreadInfo(0)`, the focus window and its owner), acquires P's client object (`AccessibleObjectFromWindow`, `OBJID_CLIENT`), then, in the moved arms, moves the keyboard focus with `SetFocus`, and finally calls `accFocus` on P's client object, counting C's messages and calls during it. Arms, 5 trials each:

- `moved`: the focus moved into C's window.
- `moved-own-child`: the focus moved into P's own child window.
- `control`: the focus left on P's window.

## Results

`results.txt`. The check named P's window, owned by P, with P in front, in 15 of 15.

- `moved`: the focus window after the move was C's, 5 of 5. `accFocus` answered `VT_EMPTY`, 5 of 5, and C took nothing.
- `moved-own-child`: the focus window after the move was P's child, 5 of 5. `accFocus` answered an object, 5 of 5.
- `control`: `accFocus` answered child id 0, the client itself, 5 of 5.

## Conclusion

Windows' standard client proxy names a child window's object as the focus when the child window is the parent's own, and names nothing when it is another process's. So the forced race does not lead the read into the other process: `focused_snapshot` then takes the client itself as the focus, P's window reported as the focused control while the focus is in C, a wrong report with no call into C. An application whose own `accFocus` answers an object served by another process would lead the read into it; none was found, and mockapp has no such object, so that case is not measured.
