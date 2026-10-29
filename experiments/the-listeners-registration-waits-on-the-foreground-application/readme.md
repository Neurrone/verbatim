# Experiment: the listener's registration waits on the foreground application

This experiment serves `docs/design/cross-process-calls-study.md`. At its start the focus listener makes its desktop-wide UIA registration (`install_desktop_subscriptions`, `crates/verbatim-outpost/src/listener.rs`), on the process's registering thread (`crates/verbatim-uia/src/subscribe.rs`): the desktop's root fetched, `AddFocusChangedEventHandler`, then the event handler group on the root. Only after it returns does the listener install its `WinEvent` hooks and say `Ready`. The architecture says the listener's registration threads make no cross-process call. The question: does the registration wait on an application, and which?

## Setup

The program is `xproc.rs` in `experiments/cross-process-calls-study-program/`, with `XP_EXP=register`, on the interactive desktop. Each trial starts a mockapp on `counts.json` and a new helper process, which makes the listener's registration (`Registration::with_parts` with the listener's four parts, focus, selection, menu opening and notifications, over the desktop) and says how long it took. The helper's process makes its first UIA use there, as the listener does at its start.

Arms, 3 trials each:

- `uia-front-stalled`: mockapp on the UIA backend, shown and brought to the front, its window thread stalled for 3 seconds as the registration is made.
- `uia-hidden-stalled`: the same, hidden and not in front.
- `uia-front-free`: in front, not stalled.
- `msaa-front-stalled`: on the MSAA backend, in front, stalled.

A second run (`XP_STALL_MS=12000`) stalled the front window for 12 seconds, 2 trials of each front-stalled arm. mockapp's counts are read after its stall ends.

## Results

`results.txt`, the 3-second run first.

- `uia-front-stalled`: the registration returned, successful, after 3,004.2 to 3,009.0 ms, 3 of 3: at the stall's end. mockapp then took 1 `WM_GETOBJECT` with `UiaRootObjectId` and a nonzero wParam (a connection), 3 with wParam 0, `GetFocus` 1, and other provider calls (`ProviderOptions` 11, `HostRawElementProvider` 4, `Navigate` 4, `FragmentRoot` 3, `GetEmbeddedFragmentRoots` 2).
- `msaa-front-stalled`: 3,001.6 to 3,004.3 ms, 3 of 3, mockapp taking 1 connection message and 2 with wParam 0.
- `uia-hidden-stalled`: 23.8 to 28.3 ms, 3 of 3, mockapp taking nothing.
- `uia-front-free`: 31.9 to 32.6 ms, 3 of 3, mockapp taking a connection, `GetFocus` and the same kinds of provider call.
- With a 12-second stall: 6,044.2 to 6,057.8 ms, successful, 4 of 4, both backends, three connection timeouts; mockapp took nothing after its stall.

## Conclusion

The listener's desktop-wide registration reads the system's focus, connecting into the foreground application and asking its provider for its focus, and waits for it: up to the application's busy time, and about 6 seconds, three connection timeouts, for an application that does not answer, after which it reports success. A busy application that is not in front holds nothing. The listener installs its `WinEvent` hooks and says `Ready` only after the registration returns, so at Verbatim's start, or when the listener is replaced, a hung foreground application keeps the listener deaf and unready for up to about 6 seconds. Which of the registration's calls makes the read, and whether the focus handler then hears the hung application's events, are not measured.
