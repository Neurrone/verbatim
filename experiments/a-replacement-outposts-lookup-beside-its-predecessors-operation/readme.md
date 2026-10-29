# Experiment: a replacement outpost's lookup beside its predecessor's operation

This experiment serves `docs/design/cross-process-calls-study.md`. When an outpost fails while its application is still running, the supervisor retires it on a thread of its own, letting the call in progress finish for up to `SHUTDOWN_LIMIT` (21 seconds), and gives the application its next outpost at once for a first failure (`Owner::outpost_failed`, `crates/verbatim-outpost/src/supervisor/owner.rs`, lines 1290 and 1291). An outpost that piled up abandoned workers fails that way, and its abandoned workers are still in calls into the application. The question: while the old outpost's remote operation waits in a busy application, what does the new outpost's first lookup by handle get?

## Setup

The program is `xproc.rs` in `experiments/cross-process-calls-study-program/`, with `XP_EXP=retire`. P is mockapp on `counts.json`, UIA backend. Two helper processes: the old outpost, which has P's window element and has set UIA up in P with one remote operation, and the new outpost, which looks P's window up with the outpost's `window_element` (the proxy rule applied).

Arms, 6 trials each: P's window thread stalled for 1 second (`stall 1000`, waited for its `stall started`), then

- `queued`: the old outpost sends a remote operation reading P's name, and 50 ms later the new outpost makes its lookup;
- `busy-only`: the new outpost's lookup alone.

## Results

`results.txt`.

- `queued`: the new outpost's lookup was refused, its element served only by UIA's own proxies, after 2,959.7 to 2,970.6 ms, 6 of 6: the stall's second and then two seconds, the connection timeout. The old outpost's operation returned P's name 1,000.9 to 1,001.3 ms after it was sent, 6 of 6.
- `busy-only`: the lookup returned P's own element after 1,001.5 to 1,004.0 ms, at the stall's end, 6 of 6.

## Conclusion

This is `docs/uia-bugs.md`'s first fault (`e1`) with the outposts' own calls: a replacement's lookup by handle into an application where its predecessor's remote operation is queued fails, by the proxy rule, after the busy period and one connection timeout more, and the predecessor's operation is unharmed. The supervisor's own timing, a predecessor stuck with workers in calls while its successor starts, is read from the code, not run: running it would need a real supervisor driving a real outpost to its abandoned-worker limit against a busy mockapp.
