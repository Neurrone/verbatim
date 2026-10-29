# Experiment: CompareElements makes no call

This experiment serves `docs/design/cross-process-calls-study.md`. The focus listener's reading threads share the elements of the focuses Core holds (`HeldElements`, `crates/verbatim-outpost/src/focus_read.rs`), and each focus's repeat check compares its sender with that element (`Uia::compare_elements`, line 1265), which can be another application's. If the comparison asked the held element's provider for its runtime id, one application's reading thread would wait on another application. The question: does `CompareElements` call either provider?

## Setup

The program is `xproc.rs` in `experiments/cross-process-calls-study-program/`, with `XP_EXP=compare`. A and B are mockapps on `counts.json`, UIA backend, `First` focused in each. The test process takes each one's focused element as the listener's registration fills it (`common::listener_focus`) and compares B's with A's, 5 times with both free and 5 with A's window thread stalled for 1.5 seconds, counting both mockapps' calls.

## Results

`results.txt`: 10 of 10 answered "different" in under 0.1 ms, and neither mockapp took a call or a message, with A stalled or not.

## Conclusion

`CompareElements` is answered in the calling process; the repeat check's comparison cannot wait on the other application. It is not an instance.
