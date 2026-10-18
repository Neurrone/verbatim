# Design: the arbitration probe as a raw WM_GETOBJECT

Status: proposed 2026-10-18, awaiting Dickson. Read-only design; nothing built.

## Decisions for Dickson

1. Replace `UiaHasServerSideProvider` with one `SendMessageTimeoutW(hwnd, WM_GETOBJECT, 0, UiaRootObjectId)`, nonzero meaning "has a provider". Recommended. It deviates from the call NVDA makes, not from the question NVDA asks.
2. The probe's wait: 3000 ms, matching the "no" UIA gives a busy window today (`e9`, `e16`). Recommended over `CALL_TIMEOUT` (5 s), which would hold a busy window's first reads 2 s longer than NVDA.
3. Take the probe out of the per-application lock. Recommended; the evidence is e34 and e38-rawpath.

## 1. Call sites today

- `has_server_side_provider` (`crates/verbatim-uia/src/probe.rs` 23): `ensure_ready`, counts one window message, then `UiaHasServerSideProvider` inside `lock::guarded_window`. So the probe takes the lock today whenever the window's provider process is the thread's application.
- Its only production caller is `arbitration::uses_uia_keeping` (`crates/verbatim-outpost/src/arbitration.rs` 330-371). It probes only when the class lists and kept verdicts give nothing. A "yes" is kept for the window's lifetime; a "no" is kept for 500 ms (`NEGATIVE_VERDICT_LIFETIME`). On a "yes" for `ConsoleWindowClass` or `SysListView32`, it runs the post-probe check.
- Through it: the outpost's `read::window_uses_uia` (`outpost/read.rs` 129, every window the worker reads) and the listener's `FocusReader::native_window` (`focus_read.rs` 860, NVDA's native check).
- Tests: `crates/mockapp/tests/arbitration.rs` calls it directly. `crates/mockapp/tests/app_lock.rs` expects the call name `UiaHasServerSideProvider` among the lock's waiters (`the_probe_waiting`, 481).

Only the body of `has_server_side_provider` changes. Callers, verdict caching and the post-probe checks stay as they are.

## 2. Semantics

The call is `SendMessageTimeoutW` with these arguments:

- `wParam` 0 and `lParam` `UiaRootObjectId` (-25).
- Flags `SMTO_ABORTIFHUNG | SMTO_BLOCK | SMTO_ERRORONEXIT`, timeout `PROBE_TIMEOUT_MS = 3000`, and `SetLastError(0)` before the call.
- `SMTO_BLOCK` matches `verbatim-ia2`'s `send`, so the caller never re-enters while it waits.
- `SMTO_ERRORONEXIT` returns at once when the window or its thread dies.
- `SMTO_ABORTIFHUNG` returns at once for a thread that has pumped nothing for 5 s. `UiaHasServerSideProvider` waits its full 3 s there, so this is cheaper and gives the same verdict.

How the result is read:

- The call failed, timed out or was refused: "no", kept for 500 ms as today. The cause goes to the debug log: timeout, hung, access denied, or gone.
- The call succeeded with result 0: "no". This is `DefWindowProc` (e34's window N).
- The call succeeded with a nonzero result: "yes". The value goes to the debug log; every observed one was `0x10000`.

The interpretation is a pure function, `verdict(sent, result, last_error) -> Probe`, so it can be unit tested.

### The LRESULT

Microsoft documents the return of `UiaReturnRawElementProvider` only as "the key for the client process to connect to the server process". It also says "Applications never send this message directly" (WM_GETOBJECT page), and that `ObjectFromLresult` is "for internal use … Neither clients nor servers should call this function". So no documented release exists, and the design calls nothing on the result.

Three pieces of evidence say no reference is created:

- The answer was `0x10000` in all 366 trials (e34 and e37 to e38), from two different servers (collide.exe and conhost.exe).
- `LresultFromObject` references are global atoms, `0xC000` to `0xFFFF`. Wine's port checks for exactly that range (`uia_provider.c`), and `0x10000` lies outside it.
- An `ObjectFromLresult` on a value it did not issue returns `E_INVALIDARG`.

This is inferred, not shown. The judge in section 5 shows it.

After a timeout, the message stays queued and the window procedure still answers it later, unread. This does no harm if no reference is created, and the leak judge covers it.

### Elevation, desktops, and false answers

- UIPI: Verbatim is `asInvoker` with `uiAccess=false`. A message to a higher-integrity window is blocked. `SendMessage` documents `GetLastError` 5 for that case, so the raw probe answers "no" at once. `UiaHasServerSideProvider` cannot connect there either. MSAA is blocked as well, so the window is unreadable in both designs. GitHub's runner is elevated, so it never meets this case. Open question 3.
- Another desktop: the hooks see only Verbatim's own desktop, so no window there reaches the probe.
- False "yes": a window procedure that returns nonzero for every object id, or an `LresultFromObject` for `UiaRootObjectId`, would read as "yes". `UiaHasServerSideProvider` probably unmarshals the result and says "no" (Wine's does). For such a window UIA would give its MSAA proxy, and the window would read through UIA's bridge instead of Verbatim's MSAA path. It would also leak an MSAA reference in that application on each probe. The guard is evidence step E1 below.
- Side effects in the application: these are the same message, so they are the same as today. Chromium and Firefox switch their accessibility on when they receive `UiaRootObjectId`.

## 3. The lock

The raw probe needs no lock:

- e34: it answered `0x10000` with a remote operation queued, where `UiaHasServerSideProvider` answered "no" in all 6 trials.
- e37-parallel and e38-rawpath: it never disturbed the lookup that followed.

`has_server_side_provider` therefore drops `guarded_window`, but keeps `ensure_ready`, so UIA's first-time setup still happens in the same place.

These calls still take the lock, unchanged from section 20.3:

- every remote operation's `Execute`;
- `ElementFromHandleBuildCache`, through `window_element` or `element_from_handle`. This includes the outpost's lookup right after the probe and `checks::is_windows_forms`;
- `ElementFromHandle`, through `window_live`. This includes `checks::console_reports_formatting`, the console's post-probe check;
- `GetFocusedElementBuildCache`;
- the classic walk's steps up into another process.

The scope shrinks by exactly one call. At an application's first sight, the outpost's wait moves from its probe to its lookup, so the time it waits is barely changed.

## 4. NVDA classification

- Preserve: the question asked, its single asking, a "no" for a window that does not answer within about 3 s, the 0.5 s life of a "no", the class lists, and the console and Windows Forms checks after a "yes".
- Replace: the call. A lone `UiaHasServerSideProvider` answers "no" falsely under the e38 trigger, and NVDA always has its own handlers registered, so NVDA is exposed to this too.
- Omit: nothing.

Documents to update:

- `docs/parity/backends.md`, "Dual-stack MSAA+UIA with per-window arbitration": add a "Different" sentence naming the raw message and e38.
- `docs/design/focus-pipeline.md`: replace section 8, "The UIA probe asks once", and remove the probe from 20.3's "Connections by window handle", 20.5 and 20.6.
- The module doc of `crates/verbatim-uia/src/lock.rs`.
- `docs/performance.md`, "What counts as a call".
- `docs/crates/verbatim-uia.md` (429, 470, 511).
- `docs/tooling/troubleshooting.md` 260.
- The roadmap item.

## 5. Costing (ledger)

- Calls per probe: 1 window message before and after. The ratchet's call counts are unchanged.
- Lock takings: one fewer per probe of a window in the thread's own application. The steady-state focus pin is unchanged, because its probe is answered from the kept verdict. Any first-sight lock pins lose one taking.
- Wake-ups: the same single delivery to the target thread. Inside the prober there may be fewer, since UIA's client machinery is no longer used for the probe; this is unmeasured.
- Latency on a started console: e37 measured about 3.5 ms raw against 9 to 11 ms for `UiaHasServerSideProvider`. A hung window costs 0 ms instead of about 3 s.
- Measure before and after: `conhost_two_windows` and `windows_terminal_commands`, ten runs each, first event to audio, following `docs/tooling/measuring.md`.

## 6. Judges

1. Unit tests for `verdict` in `verbatim-uia`: a timeout, access denied, a dead window, a result of 0, `0x10000`, and another nonzero value.
2. Mockapp, deterministic, adapting `app_lock.rs`'s two tests. The replacement outpost's probe now runs unguarded during the hold.
   - Assert: its verdict is UIA (`probe_verdict`), and its first lock waiter is `ElementFromHandleBuildCache`, which returns mockapp's own element.
   - Validation: substitute an unguarded `UiaHasServerSideProvider`; the verdict must be "no" (e34: 6 of 6).
   - The tests are renamed for the lookup.
3. Mockapp, leak judge: probe mockapp's UIA window 1000 times, then assert that mockapp's count of live root providers (a new hit counter) equals its count before the probes. `arbitration.rs`'s busy-window test keeps its assertions: one message, and "no" while the window stalls.
4. E1, evidence before switching (not a test): a scratch sweep compares both probes on every window of the measure-set applications. Any window where they disagree is examined before the change.
5. The e38 trigger itself: a mockapp version, with a window stalled before its first pump and another process holding a tooltip handler, reproduced in at most a third of trials at conhost. That is not deterministic, so it becomes a test only if an experiment (e39) shows 100%.
6. Rate: `conhost_two_windows` run 60 times. At 1 failure in 20, 60 clean runs happen by chance 4.6% of the time; 40 clean runs would happen 13% of the time. Also count probes slower than 1 s in every log.

## 7. Risks and open questions

1. The meaning of the LRESULT is undocumented, and this message is one that "applications never send". Judge 3 tests for a leak; a future Windows could change it.
2. False "yes" from windows that misbehave (E1).
3. An unelevated Verbatim with an elevated console reads "no" quickly, where today it reads "no" after a wait. Should either path try harder? (Out of scope.)
4. Whether to send this upstream to NVDA.
5. The single e38 lookup that did not answer within 1.57 s is unexplained, and the lock does not cover it.

## Amendments after the adversarial review and experiment e39 (2026-10-18)

- The review found that UiaHasServerSideProvider sends WM_GETOBJECT and then connects, so the raw message asks a narrower question. Experiment e39 (collide readme, "e39") recorded the sequence: both UIA calls send WM_GETOBJECT with a nonzero wParam, then with wParam 0, both with lParam UiaRootObjectId sign-extended (0xffffffffffffffe7), answered 0x10000, then two UiaServerConnection_HookMessage messages. In the 3 s "no" trials the window had already answered in about 3.5 ms: the wait is in UIA's client.
- Count only an answer of exactly 0x10000 as "has a provider". Three File Explorer children answer 0xffffffff80040201 where UIA says no. Send the sign-extended lParam UIA itself sends.
- Under e38's trigger, 120 trials each at 0 ms: raw probe then lookup 120 of 120 correct, median 45.6 ms in total; UiaHasServerSideProvider then lookup 120 of 120 here (39.2 ms) but 6 of 60 "no" in the hooked variant; the lookup alone 7 of 120 returned the MSAA proxy after about 5 s and misclassifies 11 Explorer children, so it is rejected as a probe. On a started console: raw then lookup 9.6 ms, UIA probe then lookup 10.2 ms.
- Cost the probe together with its lookup, as above; drop the claimed single-call saving.
- Inferred, not proven: that the raw path's lookup succeeds because it starts after the console has begun processing messages.
- Still to settle before implementing: the leak judge (count mockapp's UiaRootObjectId answers, and its handle count and private bytes, not only live providers), the flags actually used (rerun with SMTO_ABORTIFHUNG | SMTO_BLOCK | SMTO_ERRORONEXIT), a named list of frameworks for the sweep (Chromium, Firefox, Office, Java, WPF, WinForms, Qt, XAML islands, ApplicationFrameHost, DirectUI), and counting lookups that return the proxy in the rate judge.
- Awaiting Dickson's decision: replace the probe, and take it out of the per-application lock.
