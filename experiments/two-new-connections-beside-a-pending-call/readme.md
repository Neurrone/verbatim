# Experiment: two new connections beside a pending call

This experiment serves `docs/design/cross-process-calls-study.md`. `docs/uia-bugs.md`, in its first entry, records a second form of the collision: a new connection waiting in a busy application fails when another process's new connection waits there too and a third process has a call pending, "6 of 6 against mockapp and against the collide server" by `docs/design/listener-without-connections.md`, section 1. No record of that measurement is kept, so this experiment makes it again, and then makes it with the focus listener's read of a focus in a child window as one of the two connections (decision 13 of the listener design: UIA's client side connects to the parent window when a listener call returns the child window's element).

## Setup

The program is `xproc.rs` in `experiments/cross-process-calls-study-program/`, with `XP_EXP=second_form`.

- P, the parent: mockapp on `small.json`, UIA backend, hidden. P stands for a host with a provider, such as a WinUI host of WebView2.
- C, the child: mockapp on `counts.json`, UIA backend, its window made P's child with `SetParent` from the test process, `First` focused with `set-focus`.
- The listener: the test process, `OutpostUnderTest::listener_read` of the focus in C (the classic read), with an outpost for C made while C was still top-level, as the boundary test makes it.
- X, B and D: helper processes. X has P's window element and has set UIA up in P with one remote operation. B and D each look up P's window by handle with the outpost's `window_element` (one new connection each), as P's outpost does in `focus_path::find` and a focus-now read.

P's window thread is made busy one of two ways. `hold` makes the next provider call wait until `release`: X's plain name read of P is held there (mockapp prints `held GetPropertyValue`), so a third process's call is pending. `stall 300` blocks the thread with no client call in it. The release comes 300 ms after the connections are sent, since mockapp gives no evidence of a request reaching a held thread.

Arms, in each trial order:

- `two-lookups`: X's read held; B and D look up P; release.
- `lookup-listener`: X's read held; B looks up P; the listener reads the focus in C; release.
- `one-lookup`: X's read held; B looks up P; release. Control.
- `listener-only`: X's read held; the listener reads; release. Control.
- `two-lookups-stalled`: P stalled for 300 ms; B and D look up P. No third process's call pending. Control.
- `free`: nothing busy; B looks up P and the listener reads. Control.

## Results

Three runs on 2026-10-29: `run1-console.txt` (6 trials an arm, ended at its fifth arm by a setup fault since replaced), `run2.txt` (12 trials of the first three arms, 6 of the controls) and `run3.txt` (24 trials of four arms).

- `two-lookups`: in 3 of 42 trials one of the two lookups returned only UIA's proxies, which the outpost's proxy rule refuses, after 2,306.6 to 2,318.6 ms (run 1, 2 of 6; run 2, 0 of 12; run 3, 1 of 24); the other lookup returned P's own element in about 302 ms. In the other 39, both returned P's element 301.6 to 304.3 ms after the hold began, as the release let them.
- `lookup-listener`: B's lookup returned P's own element in 42 of 42. The listener's read returned the right chain (`Settings`, then C's window) in 42 of 42, after about 304 ms in 37 and after 2,308.6 to 2,309.4 ms in 5 (run 1, 1 of 6; run 2, 1 of 12; run 3, 3 of 24): one connection timeout later.
- Controls: `one-lookup` 0 failures in 36, `listener-only` no late read in 36, `two-lookups-stalled` 0 failures in 12, `free` 0 in 12 (lookups 1.7 to 3.6 ms, reads 2.6 to 3.1 ms).

The figures are from the first line of each answer; `run2.txt` and `run3.txt` hold every trial.

## Conclusion

- The second form happens here, but not every time: 3 of 42 with two outposts' lookups, against 6 of 6 recorded without a kept experiment. It needs the third process's pending call: with the thread stalled and nothing pending, 0 of 12. Why it is intermittent is not known.
- With the listener's read as one of the connections, the connection that met it was the listener's own, made by UIA's client side, in 5 of 42: the read came one connection timeout late, about 2 seconds, with the right chain. The outpost's lookup did not fail in 42. So the second form inside the listener's read is a late focus, measured, and a refused lookup of the host's outpost is not observed but not ruled out, since in `two-lookups` either connection could be the one that failed.
- Not measured: a real WinUI host of WebView2, whose child web content runs in another process; mockapp's windows stand for it.
