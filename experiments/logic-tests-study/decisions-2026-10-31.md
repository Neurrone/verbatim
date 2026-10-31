# Dickson's decisions of 2026-10-31

Taken in the session that produced `docs/design/logic-tests.md`.

- Order: the keep-id fix first; then M4.5, the foundations; then the focus pipeline rework's second build; then M4.6, the generated tests, written once against the code that stays.
- The outpost's program bookkeeping simplification moves from M6.1 into M4.5, before the tests.
- `verbatim-outpost` is split by role into outpost, listener and supervisor crates, each with its logic in a crate of its own or in `verbatim-core`, a one-time cost; the rules both readers apply go into `verbatim-reader-rules`, and the shared window code into `verbatim-win32`.
- The listener and the outpost are separate executables, split in the same build as the crates.
- The supervisor's owner thread becomes a pure state machine, in M6.2, revisiting the default of 2026-10-30 that left it impure during the ownership refactor.
- Generated tests run with fixed seeds in the gate; exploration with new seeds runs apart.
- A Linux job in CI.
- M5: the compute bound is kept by the worker's own timer, never the reducer's alarm, while the reducer keeps the program's total deadline and gives the worker the time remaining at dispatch, as recommended; an abandoned worker's module instance is replaced and its state lost, as on a trap.
- M5: global modules come into M5, reversing the decision of 2026-10-22; the tray list, the taskbar list and reading the current time move to them. A global module runs on a Core thread of its own, its reads are queries in Core's request table, and its speech is an effect Core's reducer judges.
- Determinism is enforced in the pure crates.
- The design principle on generated tests in `CLAUDE.md` asks for a reference model and exact postconditions beside invariants.
- The research notes and report are kept in `research/`.
- The console poll rule is decided when the outpost's bookkeeping simplification is designed.
- The VM deploy is removed after the roadmap is final, since the repository is no longer to determine how Verbatim is developed.

- Accepted as recommended: M5's step 0b restatements (the hook chain as a pure function, hot reload as a reducer lifecycle, module commands as queries in the request table); the speech queue as a reducer in M6.3; the M6 host protocol as a pure module with `HostedSynth`'s view of the host as a pure state; the starting points for M7 (`imbl`, `ropey` or `crop`) and M12 (synchronous `tungstenite` and `rustls`).
- No `loom` or Shuttle model of the mixer's wake rule in M6.3: not worth its cost.
- If the simulation of the reducers is built and shows value, a simulated application follows it, with its conformance checked against mockapp and recorded sessions.
