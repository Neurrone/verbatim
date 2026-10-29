# Experiment: the MSAA continuation waits on the parent window's application

This experiment serves `docs/design/cross-process-calls-study.md`. For a focus whose window is the child of another process's window, as the Settings page's content is of `ApplicationFrameHost`'s frame, the focus listener's chain stops below the boundary, and the outpost of the focus's application continues it from the parent window through MSAA (`uia_chain_onward`, `crates/verbatim-outpost/src/outpost/read.rs`, line 380, then `msaa_ancestors`), before it reports the focus. The MSAA calls carry no bound of their own (`docs/crates/verbatim-ia2.md`; `experiments/cocancelcall-ends-an-msaa-com-call-but-not-a-window-message/`); the enrichment's budget is checked only between calls. The question: when the parent window's application is busy, when is the child application's focus reported?

## Setup

The program is `xproc.rs` in `experiments/cross-process-calls-study-program/`, with `XP_EXP=msaa_onward`. Each trial has new processes: P, mockapp on `small.json`, MSAA backend, hidden; C, mockapp on `counts.json`, UIA backend, made P's child with `SetParent` after an outpost for C (`OutpostUnderTest`) was made. The listener's reader reads the focus on `First` in C with P free; then P's window thread is stalled, or not, and the read is handed to C's outpost, and the test times its report.

Arms, 3 trials each: `free`; `parent-stalled-4s`; `parent-stalled-12s`, in which the test waited 30 seconds for a report.

## Results

`results.txt`.

- `free`: the focus reported after 6.1 to 7.6 ms, with the chain P's window, C's window, `Settings`, `First`, 3 of 3.
- `parent-stalled-4s`: reported after 4,006.6 to 4,007.3 ms, the same chain, 3 of 3: at the stall's end.
- `parent-stalled-12s`: nothing reported within 30 seconds, 3 of 3, though P's stall ended after 12.

## Conclusion

The child application's focus waits for the parent window's application, the whole of its busy time, and is never reported when that lasts longer than the outpost's worker can wait. Why it is lost is inferred, not traced: the acquisition of P's client object waits up to about 10 seconds (oleacc's own limit, `e2` in the CoCancelCall experiment), the worker's handling deadline (`HANDLING_DEADLINE`) is 10 seconds, and an abandoned worker publishes nothing. So an application busy above the boundary silences the focus of the application below it, through a call the child's own outpost makes. The real Settings frame and a real WebView2 host are inferred to behave alike, not measured.
