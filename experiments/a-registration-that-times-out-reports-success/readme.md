# Experiments: a registration that times out reports success

This experiment establishes the fault recorded in `docs/uia-bugs.md` under [A registration that times out reports success](../../docs/uia-bugs.md#a-registration-that-times-out-reports-success), and the limitation [A timeout bounds each provider call, not an operation](../../docs/uia-bugs.md#a-timeout-bounds-each-provider-call-not-an-operation); both rest on the same experiment, so they share this folder. It ran on 2026-10-10 on Dickson's x64 machine (Windows 11, build 26200), in a debug build, once. Its code is `e_tt` and `e_tt_transactions` in [`../mockapp-experiments-2026-10-10/experiments.rs`](../mockapp-experiments-2026-10-10/readme.md); this folder holds only this description.

Terms: the transaction timeout is `IUIAutomation2::TransactionTimeout`, which bounds a call on an element already obtained (a transaction), as the connection timeout bounds obtaining one. A hold is mockapp's `hold` command, which makes the provider's next call of a named kind wait, inside mockapp's process, until released.

## E-TT: what a 1-second transaction timeout bounds

In a mockapp test process, against mockapp's server-side provider with the `small.json` fixture, on a desktop of its own: the transaction timeout set to 1 second, the element fetched before the hold, and then each call below made while mockapp was held:

- `CurrentName`: failed with `UIA_E_TIMEOUT` after 1,095 ms.
- `CurrentHasKeyboardFocus`: failed with `UIA_E_TIMEOUT` after 1,092 ms.
- `AddEventHandlerGroup` on the window, held in `FragmentRoot`: returned success after 1,089 ms, though the provider never answered. Whether that registration delivers anything afterwards was not tested.
- `FindFirst` over the window's descendants: failed with `UIA_E_TIMEOUT` after 13,208 ms. A search that makes many provider calls waits a full timeout for each one that the still-held thread does not answer.
- `GetFocusedElement` could not be aimed at mockapp, since nothing has the system focus on a test's own desktop; it answered the desktop at once.

The related E3 (`e3_hold_never_released`), the same file: a held `HasKeyboardFocus` read, never released, returned `UIA_E_TIMEOUT` after 22,111 ms with the default transaction timeout and after 1,092 ms with it at 1 second.

Established: the transaction timeout bounds single reads, and remote operations, which are one transaction each (`a_stalled_provider_holds_execute_until_the_transaction_timeout` in `crates/mockapp/tests/remote_ops.rs`); it does not bound an operation made of many calls; and a registration whose provider timed out reports success.

Not known: whether a registration that timed out ever delivers events.

## Rerunning

As [the mockapp experiments' folder](../mockapp-experiments-2026-10-10/readme.md) says: `cargo test -p mockapp --test experiments -- e_tt --nocapture` in a worktree at b0a1989. E-TT's registration, its read and its search are tests now, in `crates/mockapp/tests/uia_bugs/` (`docs/uia-bugs.md` names them).
