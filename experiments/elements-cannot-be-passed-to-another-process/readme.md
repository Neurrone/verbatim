# Experiments: elements cannot be passed to another process

These experiments establish the limitation recorded in `docs/uia-bugs.md` under [Elements cannot be passed to another process](../../docs/uia-bugs.md#elements-cannot-be-passed-to-another-process). They ran on Dickson's x64 machine (Windows 11, build 26200), in debug builds: E-marshal on 2026-10-10, P3 on 2026-10-18.

Terms: to marshal a COM interface is to write it into bytes (an OBJREF) that another apartment or process can unmarshal into a working proxy. A sender is the element a UIA event handler receives with an event.

## E-marshal (2026-10-10, commit f86da7f)

The `marshal` module of [`../mockapp-experiments-2026-10-10/experiments.rs`](../mockapp-experiments-2026-10-10/readme.md). An owner process (the test binary started again as a helper, on the same desktop) read mockapp's window element with the base cache request and marshaled it; the test process tried to use it. Three runs.

- `CoMarshalInterface` with `MSHCTX_LOCAL` and `MSHLFLAGS_NORMAL`, into a stream from `CreateStreamOnHGlobal`: succeeded in 0.7 to 1.3 ms, writing a 68-byte standard OBJREF. `IUIAutomationElement` is registered for standard marshaling through the type library marshaler.
- `CoUnmarshalInterface` of those bytes in the other process: `E_FAIL` (0x80004005), in all three runs.
- `CoMarshalInterThreadInterfaceInStream`: succeeded, writing a 256-byte custom OBJREF whose unmarshal class is the free-threaded marshaler, which the element aggregates. Unmarshaled in the other process: `E_UNEXPECTED` (0x8000FFFF), as expected for an in-process OBJREF.
- Within the owner process, the `MSHCTX_LOCAL` bytes unmarshaled on a single-threaded apartment thread gave a working proxy (`CurrentName` answered).
- Control: an event handler object of the test's own (`IUIAutomationEventHandler`, the same type library), marshaled the same way, unmarshaled in the other process, and a call through it with no element argument succeeded; the same call with an element argument failed with `E_FAIL`.
- For reference, the same reads made directly in the test process: `CurrentName` 85 to 118 microseconds, `HasKeyboardFocus` 76 to 95, a cached read 2 to 3, `FindFirst` 2.5 to 4.3 ms, the focus remote operation 9 to 11 ms.

## P3: holding a sender (`p3_held.py`, 2026-10-18)

With the option (c) prototype described in [`../focus-events-cannot-be-limited-to-one-application/`](../focus-events-cannot-be-limited-to-one-application/readme.md): a process holding a scoped focus registration on mockapp A kept the senders of two focus events (the `hold` flag) and read them live (`held`), on a desktop of its own.

- Live reads of them (`CurrentName`, `CurrentHasKeyboardFocus` and `CurrentProcessId`) succeeded at 0.6, 31 and 152 seconds, each in 0.2 to 1.3 ms. `HasKeyboardFocus` followed the provider's state: false once the focus moved.
- Once the element was gone (mockapp's `take-runtime-id`, which removes the node), its reads failed with `UIA_E_ELEMENTNOTAVAILABLE` (0x80040201) while the other sender still answered. After A exited, every read failed the same way, at once.

## Established and inferred

Established, on this machine: an `IUIAutomationElement` cannot be unmarshaled in a process other than the one that created it, whichever way it travels, while COM between the processes works; a sender stays usable in its own process for as long as the provider keeps the element, not for a fixed time.

Inferred: that no other route exists. None was found, documented or not; the element's own `IMarshal` was not called directly, because the workspace's `windows` crate does not enable the `Com_Marshal` feature.

## Files

- `p3_held.py` and `p3_held.out`; `proto.py`, the prototype's shared helpers, which use the prototype in `../focus-events-cannot-be-limited-to-one-application/focusproto/`.

## Rerunning

P3: build the prototype as its folder says, set `BIN` in `proto.py`, and run `python -I p3_held.py`. E-marshal: as [the mockapp experiments' folder](../mockapp-experiments-2026-10-10/readme.md) says, `cargo test -p mockapp --test experiments -- e_marshal --nocapture` at b0a1989 or f86da7f.
