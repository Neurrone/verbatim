# Experiments: the connection timeout applies to the whole process

These experiments establish the fault recorded in `docs/uia-bugs.md` under [The connection timeout applies to the whole process](../../docs/uia-bugs.md#the-connection-timeout-applies-to-the-whole-process). Both ran on Dickson's x64 machine (Windows 11, build 26200), in debug builds. Their files are kept with the series each belonged to, so this folder holds only this description.

Terms: the connection timeout and the transaction timeout are `IUIAutomation2::ConnectionTimeout` and `TransactionTimeout`, whose defaults are 2,000 and 20,000 ms. A client object is one `CUIAutomation8` instance.

## E1 (2026-10-10)

`e1_timeout_scope` in [`../mockapp-experiments-2026-10-10/experiments.rs`](../mockapp-experiments-2026-10-10/readme.md), run once in a mockapp test process. A client object on one thread read both timeouts, 10,000 and 20,000 ms (the 10,000 set by `verbatim_uia`'s client creation at the time). A client object on another thread then set both to 1,000 ms, and the first client object read 1,000 and 1,000.

Established: both settings belong to the process, not to the client object. This explained a 10-second `GetFocusedElement` in File Explorer: every client creation then set the connection timeout to 10 seconds, overwriting any shorter value another thread had set (focus pipeline section 11, which confirms the hypothesis H1 of a scratch design note, `focus-read-timeout.md`, never committed).

## h5 (2026-10-18)

`h5_read.py` and `h5_*.out` in [`../a-hung-window-named-in-a-system-focus-event-holds-a-processs-uia-events/`](../a-hung-window-named-in-a-system-focus-event-holds-a-processs-uia-events/readme.md), with the `focusproto` tool described there. A client process D, on a desktop of its own, timed an explicit read (`ElementFromHandle` and then `CurrentName`) of mockapp's comctl32 tree view window on its main client object, first while the window answered and then while its thread was stalled. Each set is one run:

- `h5_default`, no timeout set: the stalled read failed with `UIA_E_TIMEOUT` (0x80131505) after 2,014 ms.
- `h5_main500`, the main client object set to 500 ms: 514 ms.
- `h5_main500_other10000`, the main client object set to 500 ms and then another client object in the process set to 10,000 ms: 10,013 ms.
- `h5_main10000_other500`, the reverse: 508 ms.
- The read of the answering window took 13 to 16 ms in every set.

Established: the most recent setting by any client object in the process applies to every client object's reads. The `h3` set in the same folder shows the same for UIA's focus tracking, and that a client object created later that sets nothing does not reset the value (`h3_set500_thenplainclient`).

Not measured: across processes. That each process has its own value is inferred from these per-process results.
