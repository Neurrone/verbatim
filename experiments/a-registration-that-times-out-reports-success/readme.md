# Experiments: a registration that times out reports success

These experiments establish the fault recorded in `docs/uia-bugs.md` under [A registration that times out reports success](../../docs/uia-bugs.md#a-registration-that-times-out-reports-success), and the limitation [A timeout bounds each provider call, not an operation](../../docs/uia-bugs.md#a-timeout-bounds-each-provider-call-not-an-operation); both rest on E-TT, so they share this folder. E-TT ran on 2026-10-10 on Dickson's x64 machine (Windows 11, build 26200), in a debug build, once. Its code is `e_tt` and `e_tt_transactions` in [`../mockapp-experiments-2026-10-10/experiments.rs`](../mockapp-experiments-2026-10-10/readme.md); e57, which asks whether such a registration delivers events afterwards, ran on 2026-10-18 on the same machine, in a debug build, 180 trials; its code, scripts and outputs are in [`e57/`](e57/).

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

Whether a registration that timed out delivers events afterwards is e57's question, below.

## e57: when a registration that timed out starts to deliver

Terms: the stall is mockapp's `stall-until-release posted`, which keeps its window thread from taking any message until the test sends `release`; the stall's end is the moment the test has `release`'s acknowledgement. The registration's return is the moment the registering client's `AddEventHandlerGroup` call returned. The gap is the time after the stall's end in which a registration that timed out hears nothing.

Setup, one trial per test process (`e57_trial` in [`e57/e57.rs`](e57/e57.rs)): a mockapp on the UIA backend with the `small.json` fixture, on the test's own desktop. The registering client sets the connection and transaction timeouts, fetches mockapp's window element before the stall, and, once mockapp says `stall started`, registers an event handler group holding UIA's focus event (`UIA_AutomationFocusChangedEventId`, subtree scope, with the name cached) on the window. The call returns success after the connection timeout (532 to 551 ms at 500, 2,190 to 2,226 ms at 2,000, 5,514 to 5,541 ms at 5,000), while the stall still holds. The test releases the stall a set time after the return (200 ms unless stated), then raises focus events at set offsets from the stall's end. Before each one it renames a focusable node `ev<n>`, so that the handler, which reads the sender's cached name, names the event it heard. It then listens for 2 to 4 seconds after the last raise. The registering client runs either on a thread of the test process (in-process below) or in a second process, the test binary started again (second process below). The control registers with no stall.

Results:

- Every event raised in the gap was lost: none was ever delivered, in any trial, though the trials listened 2 to 4 seconds past the last raise and events raised after the gap arrived within 1 to 11 ms of being raised.
- The gap ends about 2 seconds after the registration's return, whatever the connection or transaction timeout, and not at a fixed time after the stall's end. With the stall released 200 ms after the return and a focus event every 50 ms from the stall's end, the events up to 1,750 ms after the stall's end were lost and those from 1,800 ms were heard, in all 30 trials: 5 in-process and 5 in a second process at each connection timeout of 500, 2,000 and 5,000 ms (transaction timeout 5,000 ms). The same held with the connection timeout at 2,000 ms and the transaction timeout at 500 or 20,000 ms, 3 trials each, in-process and in a second process (12 trials).
- In 10-ms steps (connection timeout 2,000 ms, 3 trials in-process and 3 in a second process), the last event lost was raised 1,780 to 1,791 ms after the stall's end and the first heard 1,790 to 1,800 ms after it: 1,981 to 2,001 ms after the registration's return.
- Released later, the gap moved with the return, not the stall: released 1,000 ms after the return, events were heard from 1,000 ms after the stall's end (the last lost at 950); released 1,700 ms after it, from 300 ms (the last lost at 250); released 3,000 ms after it, the event raised as the stall ended was heard. 3 trials of each, in-process and in a second process (18 trials).
- A stall that outlasted the return by more than 2 seconds left no gap, whatever its length: the event raised as the stall ended was heard after a stall of 4,000 or 10,000 ms past the return with the connection timeout at 500 ms, and of 6,000 ms past it at 2,000 ms (8 trials, in-process).
- The next event does not start delivery: a single event raised 1,500 ms after the stall's end was never heard, and one raised at 2,000 ms was, 5 trials each in-process and in a second process (20 trials); with events at 1,000 and 2,500 ms, the first was lost and the second heard, without the first, 5 trials each (10 trials).
- In-process and second-process registrations behaved alike in every condition.
- Control: with no stall, the registration returned in 3 ms and every event was heard within a few milliseconds of being raised (6 trials).
- No message was sent to the stalled window thread during the registration in 4 trials of the stall without `posted` (`out/x_sends_hold3000.out`); with `posted`, mockapp reported a message posted to it in every stalled trial. In 1 of 40 runs of an earlier form of the reproduction test beside the rest of the folder, which expected the posted message, a message was sent to the thread first.

Established: a registration that timed out reports success and starts to deliver about 2 seconds after its call returned, or when the stall ends if that is later; the interval follows neither timeout nor the next event. Events raised in the gap are lost, not delivered late. Once delivery starts, no later event was lost. A registration in the test process and one in a second process behave alike.

Inferred, not shown: that UIA's client makes the registration again about 2 seconds after the timeout, and that this second attempt waits in the window thread's queue until the stall ends. Whether the 2 seconds is UIA's default connection timeout, used where the process's setting is not, was not tested.

The reproduction test `a_registration_that_timed_out_misses_an_event_raised_just_after_the_stall` registers while mockapp's window thread takes no message at all (`stall-until-release quiet`), raises one focus event as soon as the thread is released, and shows that it is lost; `docs/uia-bugs.md` describes it.

The outputs are in [`e57/out/`](e57/out/), one line per trial; the 66 exploratory trials that found the conditions are in [`e57/out/explore/`](e57/out/explore/) and agree with them. The first version of the summary script misread the exploratory stream trials as hearing nothing; the corrected [`e57/onset.py`](e57/onset.py) summarises each line: the registration's time, the events lost and heard, any event heard more than 50 ms after it was raised, any heard twice, and any lost after the first one heard.

## Rerunning

E-TT: as [the mockapp experiments' folder](../mockapp-experiments-2026-10-10/readme.md) says, `cargo test -p mockapp --test experiments -- e_tt --nocapture` in a worktree at b0a1989. E-TT's registration, its read and its search are tests now, in `crates/mockapp/tests/uia_bugs/` (`docs/uia-bugs.md` names them).

e57: in a worktree at e04bdd8a, the commit that recorded it, since mockapp's stall has changed since then (it now holds every message sent to the thread, and `stall-until-release posted` reports what it held), so a run on later code is not the experiment recorded here. In that worktree, copy `e57/e57.rs` into `crates/mockapp/tests/uia_bugs/`, and add to that folder's `main.rs` the line `mod e57;`, the helper dispatch `if let Ok(window) = std::env::var(e57::REGISTRAR) { e57::registrar_process(&window); }` beside the others, and the entry `("e57_trial", e57::e57_trial)` in the test list. Add to `MockApp` in `crates/mockapp/tests/common/mod.rs` a method `pub fn line_within(&self, timeout: Duration) -> Option<String>` returning `self.lines.recv_timeout(timeout).ok()`. Build with `cargo test -p mockapp --test uia_bugs --no-run`, set `EXE` to the binary it names, and run `e57/run.sh` from the `e57` folder with the settings `e57.rs` documents, for example `E57_CTO=2000 E57_WHERE=out E57_RAISES=$(seq -s, 0 50 2500) ./run.sh 5 m_out_cto2000`. `E57_HOLD_AFTER` sets the time from the return to the release, in ms, and `E57_POSTED=0` stalls without `posted`.
