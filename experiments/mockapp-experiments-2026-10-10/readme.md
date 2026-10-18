# The mockapp experiments of 2026-10-10

The focus pipeline's design experiments of 2026-10-10 were written as a mockapp test binary, `crates/mockapp/tests/experiments.rs`, on an experiment branch that was never merged. Its code is kept here as files, not as a test: nothing in this folder is built. Several faults and limitations in `docs/uia-bugs.md` cite these experiments; each one's setup and numbers are written up in the folder of the entry it supports, listed below, and in `docs/design/focus-pipeline.md`, sections 11 and 12.11.

## The commits

- f86da7f, "Try handing a UIA element to another process with COM marshaling (experiment)": adds the `marshal` module (E-marshal), on a branch from 2cadb356 after 3ce4c226.
- 8b491dc, "Probe the focus mapping and the hung-window stall's cause (experiments)": adds the `mapping` module, `e_mapping` and `e_hung_cause`.
- b0a1989, "Measure the listener's stall from another process, and MSAA's view of UIA focus (experiments)": adds the `cross` module (`e_cross`). This branch starts from 5617d2f7 and contains 8b491dc, and the `marshal` module as 2d4a2eb0, a copy of f86da7f.

`experiments.rs` here is the file as it stands at b0a1989, which holds every experiment of the three commits; its `marshal` module is the same as at f86da7f. `experiment-branch-support.patch` is the rest of that branch, the diff from 5617d2f7 to b0a1989 without `experiments.rs`: the `[[test]]` entry in mockapp's manifest, mockapp's focus probe (`src/probe.rs`, `--focus-probe`), three helpers in `tests/common/mod.rs`, and the end-to-end experiment scenarios (`experiment_focus_routes` and the `experiment` group) that section 12.11's live coverage check used.

## The experiments in the file, and where each is written up

Each prints its measurements as lines starting `RESULT` and fails only when its setup does. All ran once, in a process of their own on a desktop of their own, against mockapp's server-side provider with the `small.json` fixture unless named.

- `e_concurrency` (E-concurrency) and `e_delivery` (E-delivery): one held application does not stall a read of another, or delay another's events in the same process. E-delivery is written up in [the hung-window fault's folder](../a-hung-window-named-in-a-system-focus-event-holds-a-processs-uia-events/readme.md); E-concurrency is in focus pipeline section 11 only, as no entry cites it.
- `e_scope` (E-scope) and `e_cost` (E-cost): [focus events cannot be limited to one application](../focus-events-cannot-be-limited-to-one-application/readme.md).
- `e1_timeout_scope` (E1): [the connection timeout applies to the whole process](../the-connection-timeout-applies-to-the-whole-process/readme.md).
- `e_tt` and `e_tt_transactions` (E-TT): [a registration that times out reports success](../a-registration-that-times-out-reports-success/readme.md), which also serves the limitation that a timeout bounds each provider call, not an operation.
- `e2_stall_within`, `e3_hold_never_released`, `e3_old`, `e4_hold_then_kill`, `e4_transaction_then_kill` and `e5_focus_read_in_held_application` (E2 to E5): what the connection and transaction timeouts bound for a stalled or killed provider. Focus pipeline section 11 holds their results; `docs/uia-bugs.md` does not cite them. E3's result is summarised with E-TT.
- `marshal::experiment` (`e_marshal`, E-marshal): [elements cannot be passed to another process](../elements-cannot-be-passed-to-another-process/readme.md).
- `e_hung_proxy` (E-hung-proxy), `e_mapping`, `e_hung_cause` and `cross::experiment` (`e_cross`): [a hung window named in a system focus event holds a process's UIA events](../a-hung-window-named-in-a-system-focus-event-holds-a-processs-uia-events/readme.md).

## Rerunning

In a scratch worktree at b0a1989 (or f86da7f for E-marshal alone), run one experiment with `cargo test -p mockapp --test experiments -- <name> --nocapture`, the name as listed in the file's `main`. The harness runs each in a child process on a desktop of its own, so the user's desktop is not touched. On today's main the file would need porting: `verbatim_uia`'s and the outpost's APIs have changed since, and `Uia::new()` no longer sets a 10-second connection timeout, which several October figures depend on (see the hung-window folder).
