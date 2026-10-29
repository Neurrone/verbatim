# Experiment: the runtime-id search enters another process's child window

This experiment serves `docs/design/cross-process-calls-study.md`. When an outpost needs a UIA node whose kept element is gone and which is not on the focus listener's last chain, `resolve_uia_element` (`crates/verbatim-outpost/src/outpost/read.rs`) searches each of its application's top-level windows for the node's runtime id: `Uia::window_live` on the window, then `Uia::find_by_runtime_id`, a `FindFirstBuildCache` over the window's subtree. The question: when the window has a child window that another process serves, as a WebView2 host or the Settings frame has, does the search go into it, and what happens when that process is busy?

## Setup

The program is `xproc.rs` in `experiments/cross-process-calls-study-program/`, with `XP_EXP=search` and `XP_PARENT` naming P's fixture and backend.

- P, the window searched: mockapp, shown, in three runs: `counts.json` on the UIA backend (a provider of its own), `small.json` on the MSAA backend, and `boundary_win32_host.json` on the `win32` backend (no provider, a UIA child window of its own on its thread).
- C: mockapp on `tree.json`, UIA backend, shown, made P's child with `SetParent` for every arm but the first.

Arms, 5 trials each, each search for the runtime id `[42, 1, 2, 3]`, which no element has, as for an element that is gone, so the search passes every element:

- `separate`: C a top-level window of its own.
- `child`: C P's child.
- `child-stalled`: C P's child, its window thread stalled for 2 seconds (`stall 2000`) for the search.
- `ancestors-query` (P on the UIA backend only): the product path. An outpost for P (`OutpostUnderTest`) reports a listener read of `First`, then mockapp disconnects `First` (every call on it fails as on an element that is gone), and the test asks the outpost for `First`'s ancestors (`Query::Ancestors`): the kept element fails, the route from the listener's chain finds nothing, and `resolve_uia_element` searches.
- `dump-tree`: the tree dump for `verbatim-inspect` (`Query::DumpTree`), 3 trials.

`hidden.txt` is a first run with both windows hidden: P on the UIA backend, no arm reached C at all, so UIA's client side passes over a hidden child window. `shown.txt` is the run with both shown.

## Results

From `shown.txt`; C's counts are the same in every trial of an arm.

- `separate`: C took nothing, 15 of 15. The search took 3.8 to 23.4 ms.
- `child`: C took 1 `WM_GETOBJECT` with `UiaRootObjectId` and a nonzero wParam, the message a UIA connection begins with, 28 other `WM_GETOBJECT`s, and 481 provider calls (`ProviderOptions` 173, `Navigate` 85, `HostRawElementProvider` 79, `GetPropertyValue` 52, `GetRuntimeId` 46, `FragmentRoot` 46) for every search, 15 of 15, whatever P's backend. The search took 10.1 to 20.5 ms.
- `child-stalled`: the same calls in C, and the search took 2,006.8 to 2,008.9 ms, 15 of 15, two seconds past the stall's start, which is the connection timeout. It then answered "not found" (`Ok(None)`), not an error, in 15 of 15.
- `ancestors-query`: the query answered `Gone` after 14.9 to 15.7 ms, and C took the same 481 provider calls and 29 `WM_GETOBJECT`s, 5 of 5.
- `dump-tree`: with P on the UIA backend the dump is one remote operation and C took nothing; with P on the MSAA backend, nothing; with P on the `win32` backend the dump's MSAA walk took C's window and client objects, 14 `WM_GETOBJECT`s, 3 of 3.

## Conclusion

- The runtime-id search goes into another process's shown child window, connects to it, and makes about ten provider calls for each of its elements: measured, for a parent with a provider, an MSAA parent and a plain Win32 parent alike, and by the product's own query path.
- When that process is busy, the search waits one connection timeout for its window, then passes over it and answers "not found". A search for an element inside the child window would answer it gone after two seconds; a search for an element outside it answers late. The crate guide's statement that a search that fails is an error does not hold here: a child window that does not answer reads as not found.
- A remote operation inside the parent's provider does not reach the child window (the UIA dump made no call in C), and the MSAA tree walk does when the parent is a plain Win32 window.
- Inferred from the same mechanism, not run: the dialog text gather (`dialog_text_of`, a walk down the dialog's children), the shell items walk under `Shell_TrayWnd` and the controlled-element search (`Uia::controlled`) go into another process's child window when one is below where they start.
