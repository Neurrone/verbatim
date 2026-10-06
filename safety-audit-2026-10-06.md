# Unsafe code safety audit

Phase 6, step 2b (phase6-design.md, "Unsafe code", item 3). Reviewed read-only at main c9114cb on 2026-10-06. The scope is every `unsafe` block, `unsafe fn`, `unsafe impl`, and `unsafe extern` in `crates/` (excluding `nvda/` and `third_party/`), plus the C++ in `crates/verbatim-gui/cpp` and the cxx bridge. The review was split into four parts by crate, each reviewed against all seven categories in the brief. A consolidating pass then re-checked every high and medium finding against the code and the API documentation. Parts A to D follow the summary unchanged, except for the corrections recorded below.

## Status

- Fixed on 2026-10-06: B1 (commit 1ff06e7; the window's process must run
  an executable of Verbatim's own file name, and every window with the
  title is considered, not only the first), B3 (42f14a3), C1 (848dcc7;
  instead of a per-session pipe name, every client checks that the
  pipe's server runs in its own session, which keeps one well-known name
  for the tools), and D1 (fab5ce4).
- The low findings were re-checked against the code after M4's Windows
  work merged (main 4a10bc3) and fixed on 2026-10-06, except where noted:
  - A1 (a8523db): `verbatim_uia::release_thread_state` drops the
    thread-local client and walker; the outpost's worker calls it before
    its thread exits. This is the conventional fix, explicit teardown
    before thread exit, rather than moving the state into the worker.
  - A2 (fe53a2b and 4d65829): the four UIA event handlers and the
    remaining `EnumWindows` callback catch a panic; the supervisor's
    callback was replaced by the outpost's window helpers.
  - A3 and A4 (fe53a2b): a SAFEARRAY from UIA must be one-dimensional,
    with elements of the expected size that own nothing; `init_mta` fails
    on a single-threaded apartment, and its documentation is corrected.
  - A5 and C9 (1a5dd8d): `inherited_pipes` rejects equal values and
    values that are not open pipes; the outpost and the synthesizer host
    report the error.
  - A6 (f052395): kept arbitration verdicts and classic-read windows
    record the window's owning thread and are ignored when it changes.
    The MSAA registry needed no change, since `acquire` already compares
    the held object.
  - A7 (2a2b145): the event thread makes its queue before reporting
    ready, and `Drop` does not wait on a thread it could not signal.
  - B4 (b0f61b6): the child-id fallback is taken only for a tree view
    older than comctl32 version 6. The sibling walk in `position_of`
    still sends handles the control returned a moment earlier; no message
    makes that walk atomic, and NVDA makes the same walk.
  - B5 (f0283bc), B6 (2527d6d), B7 (b2f6995), B8 (b70e5ec).
  - C2, C3, and C4 (1a5dd8d): child handles are inheritable only around
    `CreateProcessW`. A process creation elsewhere in Core at that same
    moment could still inherit them; the module comment says so. Core
    creates no process another way today.
  - C5 (640cfd2), C6 (a227cd8), C7 (e80cee3), C8 (638db32).
  - D2 was fixed with D1 (fab5ce4): the write copies bytes.
  - D3 (9670e39): `run_gui` refuses a second run, and the bridge's
    extern block states its thread rule. The bridge functions stay
    declared safe: they are private to the crate and reachable only from
    `run_gui` and `GuiCore` methods, and `GuiCore` is not `Sync`.
    Declaring them `unsafe fn` or giving each a `&GuiCore` argument
    would rewrite the bridge, which other work is changing now; it
    remains a possible follow-up.
  - D4: declined. Re-resolving a tray or taskbar item by name before the
    click, or invoking it through UIA, changes the dialog's behavior and
    timing, and the risk is a click on the wrong screen position, not
    memory safety; NVDA's recipe has the same weakness. It is a product
    follow-up.
  - D5 (db125d5), D6 (2fbd011).
  - D7 (2fbd011) for eSpeak NG: a sink panic stops the synthesis. For
    the GUI the abort is kept, as the finding allows: cxx aborts on a
    panic by design, and a panic in GUI state is a bug that should end
    Core visibly rather than leave it running in an unknown state.
  - The hidden-frame marker (d0ff6b6): the outposts honor it only on a
    window of Core's process, the outpost's parent. An outpost started by
    hand with `--attach` has no Core parent and never suppresses the
    frame.
- Wrong or unverifiable SAFETY comments: corrected in the commits above
  and in a045120, b42338f, and b65562d; the WASAPI `Send` reason now
  rests on the objects being free-threaded.
- Unsafe blocks replaced with existing safe wrappers: the `VARIANT`
  readers (fe53a2b and a045120), the outpost's window queries (4d65829
  and ee0b254), `verbatim-ia2`'s class-name read (2527d6d), mockapp's
  tests (936360e), and `shell_items.rs` (db125d5, which adds
  `Uia::control_view_walker`). Left as they are: `subscribe.rs`'s
  `GetRootElementBuildCache` and `nearest.rs`'s conditions, which would
  only move the unsafe call into a wrapper, and the mockapp tests'
  `FindAllBuildCache`, transaction-timeout, and MSAA calls, which have no
  public wrapper.
- `clippy::multiple_unsafe_ops_per_block` is on workspace-wide
  (b65562d). 45 blocks held more than one operation; each was split so
  that every operation has its own block and SAFETY comment. The only
  allow is on the cxx bridge module, whose shims cxx generates.
- Unsafe sites per crate after these changes, counted as below (code
  lines with `unsafe {`, `unsafe fn`, `unsafe impl`, or `unsafe extern`,
  comments excluded), with the count at 4a10bc3 in brackets. Splitting
  blocks raises a count even where unsafe operations were removed. The
  total is 548 (474).
  - verbatim-uia: 90 in src (95)
  - verbatim-agent: 73 in src (57)
  - verbatim-ia2: 60 in src (62)
  - mockapp: 43 in src (39), 32 in tests (33)
  - verbatim-outpost: 44 in src (38)
  - verbatim-app: 39 in src (18)
  - verbatim-control: 33 in src (30)
  - verbatim-audio-wasapi: 32 in src (18)
  - verbatim-process: 29 in src (24)
  - verbatim-synth-espeak: 21 in src (14)
  - verbatim-gui: 19 in src (17)
  - verbatim-input-windows: 17 in src (15)
  - verbatim-core: 0 in src, 9 in tests (unchanged)
  - verbatim-synth-host: 1 in src, 3 in tests (1 and 1)
  - verbatim-inspect: 2 in src (unchanged)
  - verbatim-synth-onecore: 1 in src (unchanged)
- Follow-ups found on the way, not fixed here: `set_clipboard_text` does
  not free its global memory when `SetClipboardData` fails, and it opens
  the clipboard with no owner window, which the documentation says makes
  `SetClipboardData` fail (B7's related notes).

## Summary

There is 1 high finding, 3 medium findings, and 26 low findings. One draft finding was withdrawn. A5 and C9 describe the same issue, `inherited_pipes` trusting raw handle values; they are counted once, under C9.

### High

- B1, `crates/verbatim-app/src/single_instance.rs` lines 97 to 136. On every start, `replace_running_instance` finds "the running Verbatim" with `FindWindowW(NULL, "Verbatim")`. That call is case-insensitive and matches any top-level window with that title. The code then posts `WM_QUIT` to the window and calls `TerminateProcess` on its process after four seconds. Only one check is made: the pid must not be this process. A File Explorer window open on a folder named `verbatim` (the repository's own folder) matches, so starting Verbatim kills explorer.exe. Fix: verify the process image (`QueryFullProcessImageNameW` against the current executable) before posting or terminating, and give the hidden frame a distinctive class to search by.
  - Consolidation check: `acquire_replacing` calls `replace_running_instance` unconditionally, before the mutex is tried, so this runs on every start. The "verify the process below" comment at line 100 promises a check that does not exist.

### Medium

- B3, `crates/verbatim-ia2/src/acquire.rs` lines 824 to 845 with `accessible.rs` line 260. The count an application returns from `accChildCount` sizes the `AccessibleChildren` buffer unchecked, before `max_nodes` is consulted. A count of `i32::MAX` asks for about 51 GB, the allocation fails, and the outpost aborts on every read of that window. A very large real list marshals every child across processes. Fix: clamp the count to the remaining `max_nodes` budget and to a constant cap.
  - Consolidation check: confirmed that `children(max)` builds a `Vec<VARIANT>` of length `max` straight from the count.
- C1, `crates/verbatim-control/src/server.rs` lines 610 to 637, with the clients at `client.rs` line 95, `verbatim-agent/src/tunnel.rs` line 184, and `server.rs` line 1032. `\\.\pipe\verbatim-control` is one name for the whole machine. It is created without `FILE_FLAG_FIRST_PIPE_INSTANCE`, and the clients connect without `SECURITY_SQOS_PRESENT`. Another account can create the pipe first, while Verbatim is not running. Its security descriptor then governs every instance, so Verbatim's owner-only DACL has no effect. The squatter can:
  - receive control traffic, including `send_keys`, which injects keystrokes into the victim's desktop;
  - impersonate the clients;
  - hang Verbatim's shutdown.
  - Fix: set `FILE_FLAG_FIRST_PIPE_INSTANCE` on the first instance, open with `SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION`, and put the session id in the pipe name.
  - Consolidation check: confirmed that `create_pipe_instance` passes only `PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED`, and that `PIPE_NAME` is a fixed constant in `protocol.rs` line 25.
- D1, `crates/verbatim-audio-wasapi/src/lib.rs` lines 431 to 463. `AudioDevice::write` is a safe method. When given fewer samples than one frame, it calls `GetBuffer(0)`, which returns `S_OK` without writing `ppData`, so the pointer stays null. The code then calls `copy_nonoverlapping` into that null pointer, which is undefined behaviour even with a count of 0. The mixer guards `frames > 0`, so no current caller reaches it, but it is undefined behaviour reachable from safe code. Fix: return early when the frame count is 0, and copy bytes rather than `f32`s (D2).
  - Consolidation check: read the block. The part D draft rated it low; it is raised to medium here because it is a soundness hole in a safe API.

### Low

- verbatim-uia and verbatim-outpost:
  - A1: COM objects in `thread_local!` are released under the loader lock.
  - A2: a panic in a UIA handler or an `EnumWindows` callback aborts the process.
  - A3: the runtime-id `SAFEARRAY`'s element type and dimension count are trusted.
  - A4: `init_mta` is never balanced, and it treats an STA thread as success.
  - A6: window-handle-keyed state relies on a destroy event that can be lost.
  - A7: `EventThread::drop` can hang.
- verbatim-ia2, verbatim-input-windows, and verbatim-app:
  - B4: `HTREEITEM` fallbacks send an integer that the target process treats as a pointer.
  - B5: a reentrant keyboard hook would panic on `borrow_mut` and abort.
  - B6: a second `WinEventHook` on the same thread clobbers the first one's callback.
  - B7: the clipboard read-back scans for a terminator without a `GlobalSize` bound.
  - B8: `unsafe impl Send for InstanceGuard` is unneeded, and its justification is incomplete.
- verbatim-process, verbatim-agent, and mockapp:
  - C2: inheritable child handles could leak into an unrelated child.
  - C3: the attribute-list buffer's alignment is not guaranteed.
  - C4: a job handle leaks on one error path.
  - C5: the agent can kill a reused pid.
  - C6: the agent acts on cached window handles.
  - C7: mockapp leaks a `SAFEARRAY` on a failed fill.
  - C8: mockapp's `accLocation` writes through out-pointers without a null check.
  - C9 (also A5): `inherited_pipes` trusts handle values from the command line.
- verbatim-gui, WASAPI, and eSpeak NG:
  - D2: the WASAPI alignment claim is undocumented.
  - D3: the bridge functions are declared safe but are GUI-thread-only, and `run_gui` has no re-entry guard.
  - D4: a stale tray-item rectangle can be clicked.
  - D5: rectangle arithmetic on values from another process can overflow.
  - D6: a long eSpeak data path silently falls back to the environment variable and the registry.
  - D7: callback panics abort the process.
- Part D's note on the hidden-frame marker is a further low item. Any process can set the marker property on its own windows to hide them from the outposts.

### Withdrawn

- The part B draft's B2 said every MSAA `VARIANT` leaks because `windows` 0.62.2's `VARIANT` has no `Drop`. That is false. `windows-0.62.2/src/extensions/Win32/System/Variant.rs` line 33 implements `Drop for VARIANT` with `VariantClear`; I confirmed this in the cargo registry source, and part A verified it independently. B2 is kept in part B as a withdrawal note.

### What the review found sound overall

- **Rust callbacks and unwinding:** a panic reaching an `extern "system"` or cxx boundary aborts the process, which is not undefined behaviour. This holds on the pinned Rust 1.99 and in cxx's generated shims.
- **COM threading:** the `windows` crate's COM interfaces are neither `Send` nor `Sync`. Elements cross threads only through `AgileReference`, and no crate has an `unsafe impl Send` or `Sync` for a COM type.
- **Window messages to other processes:** no window message in the codebase sends a pointer into Verbatim's own memory to another process.
- **Environment:** no crate calls `set_var` or `remove_var`.
- **The GUI's cxx layer:**
  - `CallAfter` may be called from other threads, as wxWidgets 3.3.3 documents.
  - Modal loops do not run pending deletions under a live handler.
  - No `RefCell` borrow is held across a call into C++.
- **The safe wrappers in `verbatim-uia/src/element.rs` and `verbatim-ia2/src/accessible.rs`:** they are sound apart from the medium item B3, which concerns how a caller sizes a buffer, not the wrapper's own contract.

## Unsafe sites per crate

These counts are code lines containing `unsafe {`, `unsafe fn`, `unsafe impl`, or `unsafe extern`, with comment lines excluded. The total is 417.

- verbatim-uia: 73 in src
- mockapp: 32 in src, 31 in tests
- verbatim-agent: 56 in src, 4 of them in `#[cfg(test)]` modules
- verbatim-ia2: 48 in src
- verbatim-outpost: 38 in src, 2 of them in a test module
- verbatim-control: 28 in src
- verbatim-process: 24 in src
- verbatim-audio-wasapi: 18 in src, 3 of them `unsafe impl`
- verbatim-gui: 17 in src, plus about 800 lines of C++ in `cpp/`
- verbatim-app: 14 in src
- verbatim-synth-espeak: 14 in src
- verbatim-input-windows: 10 in src
- verbatim-core: 0 in src, which has `forbid(unsafe_code)`; 9 in `tests/alloc.rs`
- verbatim-inspect: 2 in src
- verbatim-synth-host: 1 in src, 1 in tests
- verbatim-synth-onecore: 1 in src

The workspace sets `clippy::undocumented_unsafe_blocks` to warn, and 21 files carry `forbid(unsafe_code)`.

## Recommended order of fixes

1. B1: verify the process before killing it. This is a user-visible risk on the developer's own machine.
2. C1: first-instance flag, client SQOS, and a per-session pipe name.
3. D1 and D2 together: the zero-frame early return and a byte copy.
4. B3: clamp the child count.
5. The low items, the comment corrections, and the removable-unsafe list. Then `clippy::multiple_unsafe_ops_per_block`, as step 2b plans.


# Detailed reports by part

## Part A: verbatim-uia and verbatim-outpost

Scope: every `unsafe` block and `unsafe fn` in `crates/verbatim-uia` (src and tests) and `crates/verbatim-outpost` (src; the crate has no tests directory). Neither crate has an `unsafe impl`. Reviewed at main c9114cb, read-only.

### Summary

No high or medium findings. Seven low findings, all robustness or defence-in-depth rather than demonstrated undefined behaviour. The safe wrappers in `element.rs` hold up: their soundness rests on the `windows` crate's own invariants (an interface value is a counted live reference; a `VARIANT` value is valid and frees itself), and I confirmed the second of those in the `windows` 0.62.2 source.

Key verifications:

- `windows::Win32::System::Variant::VARIANT` in `windows` 0.62.2 has `impl Drop` calling `VariantClear` (`src/extensions/Win32/System/Variant.rs`, line 33), so every `VARIANT` returned by `GetCachedPropertyValue`, `GetCurrentPropertyValueEx`, `GetAttributeValue`, and `InitVariantFromInt32Array` is cleared exactly once. No leaks and no double clears. The `InitVariantFromInt32Array` SAFETY comment in `client.rs` (line 239) is correct, and it records a real double free that was fixed earlier.
- The same file gives safe `TryFrom<&VARIANT>` impls for `i32`, `f64`, `bool`, and `BSTR`, plus a safe `vt()` accessor. This matters for the "removable unsafe" section below.
- `#[implement]` vtable shims are `extern "system"`. On the pinned toolchain (1.99.0, `rust-toolchain.toml`), a panic that unwinds to an `extern "system"` boundary aborts the process (Rust 1.81 and later), so a panic in a UIA handler or an `EnumWindows` callback aborts the process. It is not undefined behaviour.
- Win32 COM interfaces in `windows` 0.62 are neither `Send` nor `Sync`, and neither crate has an `unsafe impl Send` or `unsafe impl Sync`. Elements cross threads only through `AgileReference` (`registry.rs`, `subscribe.rs` `Scope::Elements`, `outpost/mod.rs` `capture`), and they are resolved on MTA threads.
- Rust's `LocalKey` documentation says that on Windows the loader lock is held while a thread-local destructor runs (see finding A1).
- UIAutomationCore rejects a provider's property value of the wrong type ("Provider returned incorrect type for property, ignoring"; the Wine test suite exercises this for `UIA_RuntimeIdPropertyId`). This mitigates finding A3 but is not a documented contract.

### Findings

#### A1. COM interfaces released by thread-local destructors under the loader lock

- Where: `crates/verbatim-uia/src/checks.rs:28` (`CLIENT: RefCell<Option<Uia>>`) and `crates/verbatim-uia/src/nearest.rs:59` (`CONTEXT`, which holds an `IUIAutomationTreeWalker` and an `IUIAutomationCacheRequest` built from a per-thread `IUIAutomation`).
- Category: 2 (COM apartments and threading) and 7.
- Hole: these slots are filled on the outpost's worker threads and, for `nearest_window_handle`, on UIA's own callback threads. When such a thread exits, the `Release` calls on the UIA client objects run from Rust's thread-local destructors. On Windows those run from the TLS callback at `DLL_THREAD_DETACH`, with the loader lock held. A worker thread exits whenever the watchdog abandons it and its hung call later returns (`outpost/worker.rs`, `run` returns on `Err`), so this path runs in normal operation, not only at shutdown. Microsoft's DLL best-practice guidance rules out COM work under the loader lock. If UIAutomationCore's teardown waits on another thread that needs the loader lock (thread start or exit, or `LoadLibrary`), the exiting thread deadlocks while holding it. Every later thread start or exit in the outpost then blocks too, which freezes the outpost until the supervisor replaces it.
- Consequence: possible hang of an outpost. Not memory unsafety. I have not seen it happen.
- Severity: low (the mechanism is verified; no hang has been observed).
- Fix: keep the per-thread client and walker context in the worker's own state (the `Client` the worker already passes around) instead of in `thread_local!`, or add a `verbatim_uia::release_thread_state()` that empties both slots. The worker would call it before `run` returns. For UIA callback threads, either stop caching in TLS there, or accept the cost of creating a client per call.
- Checked: `checks.rs`, `nearest.rs`, `outpost/worker.rs` (the abandon and exit path around lines 360 to 470), the `LocalKey` documentation, and the threading claims in the `nearest.rs` doc comment.

#### A2. A panic in a UIA event handler or EnumWindows callback aborts the process

- Where: `crates/verbatim-uia/src/focus.rs:54-63`, `crates/verbatim-uia/src/subscribe.rs:288-349` (the three handlers), `crates/verbatim-outpost/src/outpost/window.rs:290` and `crates/verbatim-outpost/src/supervisor/process.rs:124` (the `visit` callbacks), and the closures they call (`listener.rs` `capture` and `Outgoing::fact`; `outpost/mod.rs` `capture` and `push`).
- Category: 3 (callbacks; unwinding across FFI).
- Hole: none of these callback bodies catches panics. `extern "system"` turns an unwinding panic into an abort, so a panic anywhere under `snapshot_parts_from_cached_element`, the registry, or tracing aborts the process. The outpost worker already wraps its work in `catch_unwind` (`worker.rs:452`); the callbacks do not. The listener is the single desktop-wide focus source (D13). If it aborts, focus tracking for every application stops until the supervisor restarts it. The current code avoids obvious panics (mutex poisoning is handled with `PoisonError::into_inner`).
- Consequence: process abort. Defined behaviour, not memory corruption.
- Severity: low.
- Fix: wrap each handler body in `std::panic::catch_unwind(AssertUnwindSafe(...))`. On a panic, log it, report a fault where a channel exists, and return `Ok(())` (or `E_FAIL`), matching the worker.
- Checked: the toolchain pin, `windows-implement` 0.60.2 shim signatures, and the callback bodies listed.

#### A3. take_i32_safearray trusts the element type and dimension count of the runtime-id array

- Where: `crates/verbatim-uia/src/com.rs:158-178`, reached from `runtime_id` (`com.rs:142`), which is called on every cached element in the listener and the outpost.
- Category: 1 (SAFEARRAY element types trusted from other processes).
- Hole: `SafeArrayGetElement` copies `cbElements` bytes into a 4-byte `i32` on the stack. If the array held 8-byte or 16-byte elements (`VT_R8`, `VT_I8`, `VT_VARIANT`), the call would write past `element`, corrupting the stack. If it held `VT_BSTR` or `VT_UNKNOWN` elements, it would allocate or AddRef into that slot and leak. Only dimension 1 is read, so a multi-dimensional array would fail per element (harmless). The values ultimately come from the provider's `GetRuntimeId` in another process. UIA core validates property types and composes the runtime id itself for HWND-based providers, so a mismatched array should never arrive. That is UIA's implementation behaviour, not a documented contract of `IUIAutomationElement::GetRuntimeId`.
- Consequence: stack memory corruption, but only if UIA's type validation were bypassed.
- Severity: low (defence in depth).
- Fix: before reading, check `SafeArrayGetDim(array) == 1` and that `SafeArrayGetVartype(array)` is `VT_I4` (or that `SafeArrayGetElemsize(array) == 4`). Return an empty vector otherwise, still destroying the array.
- Checked: the `com.rs` code; UIA's "incorrect type for property, ignoring" behaviour (Microsoft's UIA event-log descriptions; the Wine `uiautomationcore` tests of `UIA_RuntimeIdPropertyId` types).

#### A4. init_mta is never balanced and treats an STA thread as success

- Where: `crates/verbatim-uia/src/com.rs:61-70`, called from `create_client` (`client.rs:715`) and `outpost/worker.rs:384`.
- Category: 2 (CoInitializeEx and CoUninitialize pairing).
- Hole: every successful `CoInitializeEx` adds one to the thread's COM init count, and nothing ever calls `CoUninitialize` for these (the `ensure_ready` setup thread is the only balanced one). `create_client` runs `init_mta` on every `Uia::new`, so a thread that builds several clients stacks up several unbalanced inits. This is harmless in practice because `CoIncrementMTAUsage` (`client.rs:675`) pins the MTA for the life of the process. The doc comment's "Idempotent per thread" is inaccurate. Separately, `RPC_E_CHANGED_MODE` is accepted, so a `Uia` created on an STA thread succeeds and is bound to that STA. The crate's model (architecture section 4: UIA clients live in the MTA) is then silently broken for that client. Today every caller is a fresh worker, registration, or setup thread, so no STA caller exists in these two crates.
- Consequence: no undefined behaviour. A future STA caller would get a client whose event registrations need that thread's message pump.
- Severity: low.
- Fix: correct the doc comment (the call adds an init that is deliberately never undone, because the MTA is pinned). Have `create_client` fail, or debug-assert, when the thread is in an STA, so that the "client lives in the MTA" invariant holds.
- Checked: `com.rs`, `client.rs` (`ensure_ready` and `create_client`), and the call sites in the outpost.

#### A5. inherited_pipes call sites trust handle values from the command line

- Where: `crates/verbatim-outpost/src/main.rs:52` and `main.rs:64` (the `unsafe fn` itself is in `verbatim-process`, outside this part).
- Category: 7 (process handles).
- Hole: the SAFETY comments say the values name inherited pipe ends owned by exactly one `File`. Nothing checks this. With `--pipe-in N --pipe-out N`, two `File`s own one handle and it is closed twice. The second close can close an unrelated handle that has since reused the value. A value naming some other handle the process holds would be taken over and closed in the same way. Only the supervisor launches the process with these arguments, so this needs a malformed launch.
- Consequence: an I/O-safety violation, and a double close that can hit an unrelated handle.
- Severity: low.
- Fix: in `parse_args` or `inherited_pipes`, reject equal values and check `GetFileType(handle) == FILE_TYPE_PIPE` before taking ownership.
- Checked: `main.rs` and `verbatim-process/src/lib.rs:460-477`.

#### A6. HWND-keyed outpost state relies on a destroy event that can be lost

- Where: `crates/verbatim-outpost/src/outpost/worker.rs:768-772` (forgetting on `EVENT_OBJECT_DESTROY`), the arbitration verdict cache in `arbitration.rs`, and the per-window MSAA registry. The listener resolves a pid at event time (`listener.rs:432`), and the outpost later acts on the same raw HWND.
- Category: 5 (window-handle reuse).
- Hole: verdicts and nodes are keyed by HWND and cleared only when the out-of-context destroy event arrives. If that event is dropped (out-of-context WinEvents are not guaranteed), a recycled HWND inherits the old window's arbitration verdict and nodes. Neither crate ever posts, sends, or focuses a cached HWND. The only messages sent are `WM_GETOBJECT` (through `UiaHasServerSideProvider`) and `WM_NULL` (`probe.rs:87`), both harmless to a wrong window. So the worst case is reading a window through the wrong API, or reporting a stale node.
- Consequence: wrong-object reads. No wrong-object actions.
- Severity: low.
- Fix: store the window's owning thread id, or creation identity (thread id plus class), alongside each HWND-keyed entry, and re-check it with `GetWindowThreadProcessId` on lookup.
- Checked: `grep` for `SendMessage`, `PostMessage`, `SetForegroundWindow`, `SetFocus`, and `ShowWindow` over both crates (only `probe.rs`'s `SendMessageTimeoutW(WM_NULL)` matched), and the `msaa_event` destroy handling.

#### A7. EventThread's Drop can hang if WM_QUIT cannot be posted

- Where: `crates/verbatim-outpost/src/event_thread.rs:48-62`.
- Category: 7.
- Hole: `Drop` ignores the result of `PostThreadMessageW` and then joins unconditionally. `PostThreadMessageW` fails if the target thread has no message queue, or if `thread_id` is 0 because the thread died before sending its id. `WinEventHook::install` always calls `SetWinEventHook` (a user32 call that creates the queue) before it can fail, and the subscription lists are constant and non-empty, so the post succeeds today. The guarantee is incidental, not explicit.
- Consequence: a hang on drop, theoretical today.
- Severity: low.
- Fix: call `PeekMessageW(..., PM_NOREMOVE)` at the start of `event_thread_main` to force the queue to exist, as Microsoft's `PostThreadMessage` documentation recommends, and only join when the post succeeded.
- Checked: `event_thread.rs` and `verbatim-ia2/src/hook.rs:161-196`.

### SAFETY and doc comments that are wrong or unverifiable

- `com.rs:54-61`: the `init_mta` doc says "Idempotent per thread". Each successful call adds a COM init that must be balanced, and none is (A4).
- `com.rs:143-144` (`runtime_id`) and `com.rs:162-164`: "returns a SAFEARRAY of i32" and "a valid, caller-owned SAFEARRAY of i32" assert the element type without checking it. This relies on undocumented UIA validation (A3).
- `main.rs:50-51` and `main.rs:62-63`: "each is owned by exactly one File" is not checked; equal arguments break it (A5).
- `element.rs:306` and `element.rs:474`: "`index` is within the array's length" is true but not a safety precondition; an out-of-range index just fails with `E_INVALIDARG`. Harmless, but it suggests a precondition that does not exist.
- `element.rs:86-87` (`cached_string`) and `com.rs:72-73`: the docs say `None` when the value is "not a string". `VariantToStringAlloc` coerces numbers and booleans to text, so an integer property reads as its decimal string. This is a doc accuracy point, not safety.
- `event_thread.rs:50`: "WM_QUIT ends the message loop" holds only if the post succeeds, which is never checked (A7).
- `focus.rs:134-136`: "dropping our `cache` handle after registration is safe" is correct, but the reason given (the handler AddRefs) refers to the wrong object. UIA AddRefs both the handler and the cache request it keeps. A minor wording issue.

All other SAFETY comments in both crates were checked against the code and the API contracts (`GetAncestor`, `GetClassNameW`, `InternalGetWindowText`, `GetGUIThreadInfo` with `cbSize` set, `GetPropW`, `OpenProcess` and `CloseHandle` pairing, `QueryFullProcessImageNameW` length in/out, the `EnumWindows` `lparam` lifetime, `UiaHasServerSideProvider`, and `CoIncrementMTAUsage`), and they are accurate.

### Unsafe that existing safe code could remove

- `com.rs` `variant_i32`, `variant_f64`, `variant_bool`, `variant_optional_bool`, and `variant_string` (5 `unsafe fn`s with 7 inner unsafe sites), plus the 6 `unsafe` blocks that call them in `element.rs` (lines 59, 67, 75, 83, 91, and 243). The `windows` crate's own safe API covers every case, because it already treats a `&VARIANT` as valid:
  - `variant_i32` becomes `i32::try_from(&value).ok()`.
  - `variant_f64` becomes `(value.vt() == VT_R8).then(|| f64::try_from(&value).ok()).flatten()`.
  - `variant_optional_bool` becomes a `value.vt() == VT_BOOL` check, then `bool::try_from`.
  - `variant_bool` becomes `bool::try_from(&value).unwrap_or(false)`.
  - `variant_string` becomes `BSTR::try_from(&value)` (the workspace already enables `Win32_System_Com_StructuredStorage`).
  - Check one behaviour: `bool::try_from` uses `VariantToBoolean`, which coerces some non-boolean types, as `VariantToBooleanWithDefault` does today.
- `listener.rs:434` (`window_pid`) duplicates `outpost/window.rs` `window_owner` (private). Making that `pub(crate)` removes the block.
- `supervisor/process.rs:124-155` (`application_is_hung`, 3 sites) re-implements `outpost/window.rs` `top_level_windows` plus `IsWindowVisible` and `IsHungAppWindow`, which `window.rs` already wraps (`window_is_visible` is private and `window_is_hung` is `pub(super)`). Widening their visibility removes the callback and both blocks.
- `arbitration.rs:205` (`GetAncestor(GA_ROOT)`) duplicates `outpost/window.rs` `top_level_of`. The dependency currently runs the other way (`window.rs` imports `window_class_name` from `arbitration`), so moving `top_level_of` into `arbitration.rs`, or into a shared module, removes it.
- `subscribe.rs:234` (`GetRootElementBuildCache`) could become a `Uia` method next to `root_element`. `nearest.rs:48` calls `CreatePropertyCondition` on a raw client where `Uia::property_condition` exists (it would need `create_client` to return a `Uia`). Both just move the unsafe into one documented wrapper, in line with phase 6 item 2.

### Unsafe counts (code lines only, comment lines excluded)

- verbatim-uia, src: 73 lines: 67 `unsafe` blocks or expressions, 6 `unsafe fn` (the 5 `variant_*` and `take_i32_safearray`), 0 `unsafe impl`. By file: `element.rs` 34, `com.rs` 17, `client.rs` 11, `subscribe.rs` 4, `focus.rs` 2, `probe.rs` 2, `cache.rs` 2, `nearest.rs` 1.
- verbatim-uia, tests: 0.
- verbatim-outpost, src: 38 lines: 36 blocks, 2 `unsafe extern "system" fn` (`EnumWindows` callbacks), 0 `unsafe impl`. Two of the blocks are in `outpost/window.rs`'s `#[cfg(test)]` module (`CreateWindowExW` and `DestroyWindow`), leaving 36 lines (34 blocks and 2 functions) in non-test code. By file: `outpost/window.rs` 23, `supervisor/process.rs` 6, `event_thread.rs` 4, `main.rs` 2, `arbitration.rs` 2 (lines 205 and 299), `listener.rs` 1.
- verbatim-outpost, tests: no tests directory.

Out-of-scope note for the other parts: `verbatim-gui/src/shell_items.rs` creates a `verbatim_uia::Uia` on short-lived worker threads, which can be abandoned, so finding A1's TLS concern does not apply there (it uses no TLS client). `verbatim-ia2/src/hook.rs` keeps its WinEvent callback in a `thread_local!` (`CALLBACK`), so it is dropped under the loader lock too. That is a boxed closure holding `Arc`s (no COM), so it is fine unless the closure owns COM objects.


## Part B: verbatim-ia2, verbatim-input-windows, verbatim-app, verbatim-core tests, verbatim-inspect

Reviewed at c9114cb. Every `unsafe` line in these crates was read in context. The `acquire`, `registry`, `map`, `calls`, and `com` modules of `verbatim-ia2` hold no `unsafe` and were read only where they call the wrappers. The `windows` crate version is 0.62.2, and its generated source was read for the `VARIANT` and `IAccessible` definitions.

### Findings

#### B1. A running Verbatim is found by a case-insensitive title match on any top-level window, then that window's process is sent WM_QUIT and terminated (high)

- Where: `crates/verbatim-app/src/single_instance.rs`, lines 97 to 136 (`replace_running_instance`).
- Category: 5 and 7 (wrong-object action on a window handle; process handles).
- Hole: `FindWindowW(NULL, "Verbatim")` matches any top-level window in the session whose title is "Verbatim" in any letter case. Microsoft's documentation for `FindWindowW` says it "does not perform a case-sensitive search", and with a null class name "it finds any window whose title matches". The only check afterwards is that the owning pid is not zero and not this process. A File Explorer window open on a folder named `verbatim` (the repository's own folder name, and File Explorer titles a folder window with the folder's name) matches. Starting Verbatim then posts `WM_QUIT` to that Explorer window, waits four seconds for `explorer.exe` to exit, which it does not, and calls `TerminateProcess` on `explorer.exe`, killing the shell and the taskbar. Any other application with a window titled "Verbatim" is killed the same way. The module comment says "match by title alone and verify the process below", but no such verification exists.
- Consequence: termination of an unrelated user process, possibly the shell. Also, a matching unrelated window that comes first in z-order hides a real running Verbatim, which is then not replaced; startup then fails after the two-second mutex wait.
- Checked: the code, `crates/verbatim-gui/cpp/gui.cpp` lines 55 to 66 (the hidden frame's title is the rendezvous), and the `FindWindowW` documentation on Microsoft Learn.
- Fix: after `OpenProcess`, verify the process is a Verbatim: compare `QueryFullProcessImageNameW` against the current executable's path (or at least its file name), and open with `PROCESS_QUERY_LIMITED_INFORMATION` added. Also narrow the search: give the hidden frame a class name of its own (or match wxWidgets' class as NVDA does with `wxWindowClassNR`), and enumerate with `FindWindowExW` so a non-Verbatim window earlier in z-order does not hide the real one. Only post `WM_QUIT` and terminate after the image check passes.

#### B2. Withdrawn: MSAA VARIANTs are not leaked

The part B draft reported that every `VARIANT` returned by MSAA leaks because `VARIANT` has no `Drop`. That is wrong: `windows` 0.62.2 implements `Drop for VARIANT` calling `VariantClear` in `src/extensions/Win32/System/Variant.rs` (line 33), confirmed in the registry source during consolidation. `related` clones the `pdispVal` reference, and the `VARIANT`'s own reference is released when it drops; the `AccessibleChildren` buffer is a `Vec<VARIANT>` whose elements are each cleared on drop (unfilled ones are `VT_EMPTY`). String roles and `VT_UNKNOWN` selections are freed the same way. No finding remains.

#### B3. The child count used to size AccessibleChildren's buffer is taken from the application unchecked (medium)

- Where: `crates/verbatim-ia2/src/acquire.rs`, lines 824 to 845, calling `Accessible::children` at `crates/verbatim-ia2/src/accessible.rs` line 260.
- Category: 1 and 4 (buffer sizes trusted from other processes).
- Hole: `accChildCount` is answered by the application. Any positive value is passed straight to `children(max)`, which allocates `max` `VARIANT`s (24 bytes each) and asks `AccessibleChildren` for all of them, before `limits.max_nodes` is consulted at line 849. A buggy or hostile application answering `i32::MAX` makes the outpost request about 51 GB; the allocation fails and Rust aborts the outpost. A real control with a very large count (a virtual list view with millions of items) costs a large allocation and an `AccessibleChildren` that marshals every child across processes, of which all but `max_nodes` are discarded.
- Consequence: crash (abort) of the outpost serving that application, repeated on every read; or long stalls up to the worker deadline.
- Checked: the caller in `acquire.rs` and the walk limits (`max_nodes`, lines 747 to 780).
- Fix: clamp the request to what the walk can still use, `child_count.min(limits.max_nodes.saturating_sub(state.visited))` (and an absolute constant cap), in the caller; optionally also cap inside `children`.

#### B4. Tree view navigation sends integers that comctl32 treats as pointers into the target process (low)

- Where: `crates/verbatim-ia2/src/acquire.rs` lines 386 to 395 (`htreeitem_for_acc_id`'s fallback) and 1145 to 1155 (`position_of`'s sibling walk); `crates/verbatim-ia2/src/window.rs` lines 155 to 167.
- Category: 4 (cross-process SendMessage).
- Hole: an `HTREEITEM` is a pointer to the item's structure inside the application. When `TVM_MAPACCIDTOHTREEITEM` answers zero (on comctl32 version 6 that means the child id no longer maps, for example the item was just deleted), the code falls back to sending the child id itself, a small integer, as the `HTREEITEM` of `TVM_GETNEXTITEM`. The sibling walk also reuses handles across several messages while the application may delete items in between. Whether retail comctl32 validates the handle before dereferencing it is not documented, so a crash of the target application could not be confirmed; the behavior matches NVDA's `sysTreeView32` module, which makes the same fallback. Nothing in Verbatim's own process is at risk: the wrapper's claim that no pointer parameter is sent holds for this process.
- Fix: take the fallback only for a control older than version 6 (ask with `CCM_GETVERSION`, an integer-only message), and treat a zero mapping on version 6 as "no such item".
- The SAFETY comments at `window.rs` lines 156 to 157 and 164 to 165 ("which the control only looks up") are not verifiable; see the list below.

#### B5. Reentrancy in the low-level keyboard hook aborts Verbatim (low)

- Where: `crates/verbatim-input-windows/src/lib.rs`, lines 249 to 281.
- Category: 3 (callbacks and reentrancy, unwinding).
- Hole: the hook holds `HOOK_STATE.borrow_mut()` while calling the speech effect closure. Low-level hook calls are delivered to the installing thread whenever it waits in a way that dispatches sent messages. If the closure ever made a call that dispatches (a cross-apartment COM call, `SendMessage`), a second key would re-enter `keyboard_hook`, `borrow_mut` would panic, and a panic in an `extern "system"` function aborts the process (Rust 1.81 and later abort instead of unwinding across this ABI, so this is not undefined behavior). Today the closure only sends on an unbounded crossbeam channel (`verbatim-speech/src/manager.rs` lines 159 to 166), so this cannot happen now.
- Fix: use `try_borrow_mut` and answer `Pass` (calling `CallNextHookEx`) when it fails, and state in `SpeechEffectFn`'s doc that it must not pump messages.

#### B6. A second WinEventHook on one thread clobbers the first one's callback (low)

- Where: `crates/verbatim-ia2/src/hook.rs`, lines 154 to 214.
- Category: 3.
- Hole: `install` overwrites the thread-local `CALLBACK` unconditionally, and both a failed `install` and `Drop` clear it. Two hook sets on one thread would deliver the first set's events to the second callback, and dropping either silences both. No caller installs two today (`verbatim-outpost/src/event_thread.rs` installs once per event thread). Memory-safe; a correctness trap. `WinEventHook` is not `Send` (it holds `HWINEVENTHOOK`, a raw pointer), so `UnhookWinEvent` does run on the installing thread as its documentation requires.
- Fix: refuse a second install on a thread that already has a callback, or key the callbacks by hook handle.

#### B7. Clipboard read-back scans for a terminator without a bound (low)

- Where: `crates/verbatim-app/src/clipboard.rs`, lines 42 to 64 (`clipboard_text`).
- Category: 1 (slices from foreign pointers and lengths).
- Hole: the loop walks `CF_UNICODETEXT` data until a zero unit, with no limit from `GlobalSize`. The data belongs to whichever process last set the clipboard. `clipboard_text` is only called right after `set_clipboard_text` succeeds, but a clipboard monitor (or any process) can replace the contents between the two `OpenClipboard` calls; data without a terminator makes the scan read past the allocation: an out-of-bounds read, an access violation if it reaches an uncommitted page.
- Fix: bound the scan by `GlobalSize(memory) / 2`.
- Related, not a safety issue: when `SetClipboardData` fails, the `GlobalAlloc` block is not freed (`GlobalFree` needed on that path). Also, Microsoft's `OpenClipboard` documentation says that after `OpenClipboard(NULL)`, `EmptyClipboard` sets the owner to null, "this causes SetClipboardData to fail"; the copy evidently works in practice, but passing a window of this process (the hidden frame) would follow the documented contract. Both are follow-ups.

#### B8. `unsafe impl Send for InstanceGuard` is unneeded and its justification is incomplete (low)

- Where: `crates/verbatim-app/src/single_instance.rs`, lines 47 to 49.
- Category: 2 and 7.
- Hole: a mutex is owned by a thread, and `ReleaseMutex` fails with `ERROR_NOT_OWNER` on any other thread. The SAFETY comment speaks only of `CloseHandle`. If the guard were moved to and dropped on another thread, the mutex would stay owned until the main thread exits, and the next instance would see `WAIT_ABANDONED` and log a crash that did not happen. The guard is in fact a local of `main` (`main.rs` line 95) and never moves, so the impl is unused.
- Fix: delete the impl, which makes the guard `!Send` and the single-thread requirement compiler-checked.

### Checked and found sound

- `Accessible::from_event` and `client_of_window`: out-parameters are locals, `Option<IAccessible>` has the interface pointer's layout, and a stale window handle makes the call fail. The child `VARIANT` from `AccessibleObjectFromEvent` is `VT_I4`, which owns nothing.
- `Accessible::identity_string`: the `size_is` marshalling of `GetIdentityString` allocates exactly `length` bytes, the slice is copied before `CoTaskMemFree`, and the buffer is freed once on the success path; on failure the out-pointer is not read.
- `related`: the type tag of a `VARIANT` produced by COM marshalling matches its contents, so the union reads are sound; there is no leak (B2 withdrawn).
- `AccessibleChildren`: the slice length bounds what it writes, and `obtained` is clamped to the buffer.
- Thread and apartment: `IAccessible` in the `windows` crate is neither `Send` nor `Sync`, so the compiler confines each object to its thread; the registry keeps only `AgileReference`s and resolves them on use.
- `win_event_proc`: does only local calls (`GetClassNameW` with a 64-unit buffer, clamped), and calls the callback under a shared borrow, so a reentrant delivery is harmless. A panic there aborts rather than unwinding (see B5).
- The keyboard hook dereferences `KBDLLHOOKSTRUCT` only for `HC_ACTION`, as the `LowLevelKeyboardProc` contract provides; the hook state is installed before the thread pumps, so the "never runs against an empty thread-local" comment holds even though the hook is set first. `InputHook::drop` posts `WM_QUIT` while still holding the `JoinHandle`, so the thread object is open and its id cannot have been reused.
- `datetime.rs`: the two-call sizing pattern, and `string_from` slices with `get`, so a written count beyond the buffer yields `None`.
- `foreground_pid`, `report_toggle_key`, `flight_dump::to_utc_systemtime`, `verbatim-inspect/src/timestamp.rs`: plain local calls with local out-parameters.
- `verbatim-core/tests/alloc.rs`: forwards to `System` unchanged; `ALLOCATED` is a `const`-initialized `Cell`, so counting never allocates or registers a destructor, and `try_with` covers thread teardown.
- No environment mutation (`set_var`) and no `CreateProcess`-style handle inheritance in these crates.

### SAFETY comments that are wrong, misleading, or unverifiable

- `crates/verbatim-ia2/src/accessible.rs` lines 384 to 386, 200, and 209: correct as written (B2 withdrawn); they could add that the `VARIANT`'s own `Drop` (`VariantClear`) releases what it owns, since the draft shows a reader can miss that.
- `crates/verbatim-ia2/src/window.rs` lines 156 to 157 and 164 to 165: "which the control only looks up" is not verifiable; comctl32 treats an `HTREEITEM` as a pointer in the target process (B4). The module doc's claim at lines 11 to 13 is correct but only about this process.
- `crates/verbatim-app/src/single_instance.rs` lines 47 to 48: covers `CloseHandle` but not `ReleaseMutex`'s owning-thread requirement (B8).
- `crates/verbatim-app/src/single_instance.rs` line 100: "verify the process below" promises a check that is not made (B1).
- `crates/verbatim-app/src/clipboard.rs` lines 42 to 44: "its null-terminated UTF-16 contents" assumes a property of data written by another process (B7).
- `crates/verbatim-inspect/src/timestamp.rs` line 42: the comment begins "SAFETIME is converted with the zone information", a garbled word that reads like a SAFETY comment but is not one; it should say "The SYSTEMTIME is converted".
- `crates/verbatim-input-windows/src/lib.rs` lines 154 to 156: "It does not fail in practice" is a remark, not a safety argument; the call's safety does not depend on it (fine, but worth rewording).

### Unsafe blocks an existing safe wrapper could replace

- `crates/verbatim-ia2/src/hook.rs` lines 237 to 242: the `GetClassNameW` closure in `is_wanted` duplicates `crate::window::class_name` (`window.rs` line 61), which can be called directly, removing that block.
- `crates/verbatim-app/src/main.rs` line 587 (`foreground_pid`) and `crates/verbatim-app/src/single_instance.rs` line 111 duplicate the `GetWindowThreadProcessId` pattern of `verbatim_ia2::window::focused`, but that module is `pub(crate)`, so no existing public wrapper applies; making a small public window helper would be new work.
- `crates/verbatim-app/src/flight_dump.rs` line 95 and `crates/verbatim-inspect/src/timestamp.rs` line 38 perform the same `FileTimeToSystemTime` conversion (the code says so); there is no shared wrapper today.

### Unsafe counts (lines containing the `unsafe` keyword, comment lines excluded)

- verbatim-ia2: src 48, tests 0 (accessible.rs 25, window.rs 15, hook.rs 6; the rest of the crate has none).
- verbatim-input-windows: src 10, tests 0.
- verbatim-app: src 14 (single_instance.rs 7, clipboard.rs 2, datetime.rs 2, flight_dump.rs 1, main.rs 2), tests 0.
- verbatim-core: src 0 (`#![forbid(unsafe_code)]`), tests 9 (all in tests/alloc.rs).
- verbatim-inspect: src 2 (timestamp.rs), tests 0.


## Part C: verbatim-agent, mockapp, verbatim-process, verbatim-control, verbatim-synth-host

Reviewed at c9114cb. Every `unsafe` block, `unsafe fn`, and `unsafe impl` in these five crates' `src` and `tests` was read in context. No `std::env::set_var` or `remove_var` exists anywhere in `crates/`, so the Rust 2024 environment-mutation hazard does not arise. The toolchain is Rust 1.99, so a panic that reaches the boundary of any `extern "system"` function in these crates (window procedures, `EnumWindows` callbacks, the `#[implement]` COM shims in mockapp) aborts the process rather than unwinding into foreign frames: a crash, never undefined behavior.

### Findings

#### C1. The control-plane pipe can be squatted, and its clients allow impersonation (medium)

- Where: `crates/verbatim-control/src/server.rs:610-637` (`create_pipe_instance`, used for the first instance at line 853 and every later one at line 865); the clients at `crates/verbatim-control/src/client.rs:95-98` (`OpenOptions` with no `security_qos_flags`), `crates/verbatim-agent/src/tunnel.rs:184-194` (`CreateFileW` with `FILE_FLAG_OVERLAPPED` only), and `ControlServer::drop`'s own wake-up open at `server.rs:1032-1036`.
- Category: 7 (named pipe security).
- The hole: `CreateNamedPipeW` is called without `FILE_FLAG_FIRST_PIPE_INSTANCE`, and `PIPE_NAME` (`\\.\pipe\verbatim-control`, `protocol.rs:25`) is one machine-wide name, not per session. For named pipes, the security descriptor given when the first instance is created applies to every instance; the one passed with later instances is ignored. A process of another account on the machine (another logged-on user, or a service) that creates `\\.\pipe\verbatim-control` first, with a permissive DACL, while Verbatim is not running, makes Verbatim's later `CreateNamedPipeW` succeed as an additional instance of the attacker's pipe. Then:
  - The owner-only DACL Verbatim builds (`D:P(A;;GA;;;<sid>)`) is not in force, so the other account can connect to Verbatim's own instances and issue control requests, including `send_keys`, which Verbatim executes with `SendInput` on the victim's desktop: keystroke injection across accounts.
  - The victim's clients (`verbatim-inspect`, the agent's tunnel, Verbatim's own shutdown wake-up) can land on the attacker's instance. None of them sets `SECURITY_SQOS_PRESENT`, so the default impersonation level is `SecurityImpersonation`, and the attacker's server can call `ImpersonateNamedPipeClient` and act with the victim's token. None of them checks who the server is.
  - Lesser effects: if the squatter's pipe was created with a maximum instance count of 1, Verbatim's control plane fails to start; and `ControlServer::drop`'s wake-up open can connect to the squatter instead of the accept thread, leaving `thread.join()` blocked forever on Verbatim's `ConnectNamedPipe`, which hangs shutdown.
  - Separately from squatting: the same user in two sessions (console and RDP) running two Verbatims gets two instances of one pipe, so a client can reach the other session's Verbatim.
- Consequence: cross-account keystroke injection into, and token impersonation of, the user running Verbatim. It needs a local attacker on another account and a time when Verbatim is not running. The `--secure` instance has no control plane, so the secure desktop is not exposed.
- Fix: create the first instance (the one in `start_on`) with `FILE_FLAG_FIRST_PIPE_INSTANCE`, and fail the control plane loudly when that returns `ERROR_ACCESS_DENIED`. Later instances keep the flag off. On every client open, set `SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION`: in `client.rs` through `OpenOptionsExt::security_qos_flags(SECURITY_IDENTIFICATION)`, and in `tunnel.rs` by OR-ing both flags into the `CreateFileW` attributes. Optionally, have clients check the server's process with `GetNamedPipeServerProcessId` and compare its token user. Put the session id in the pipe name (for example `verbatim-control-<session>`), which fixes the two-session case and narrows the squatting target. Also bound the wake-up loop in `drop` so it cannot hang if the wake-up never reaches the accept thread.
- What I checked: the first-instance rule and the effect of `FILE_FLAG_FIRST_PIPE_INSTANCE` against CyberArk's write-up of the RDP named-pipe squatting vulnerability and the Klocwork SV.PIPE checks, which are consistent with the `CreateNamedPipe` documentation; the Rust `OpenOptionsExt` documentation ("By default `security_qos_flags` is not set"), together with Windows' default of `SecurityImpersonation` when `SECURITY_SQOS_PRESENT` is absent; the manifest (`verbatim.exe.manifest`: `uiAccess="false"`, `asInvoker`), which rules out an elevation-to-UIAccess variant; and that the accept loop always keeps one instance alive, so only the very first creation can be raced.

#### C2. A child's inheritable pipe and log handles can leak into an unrelated child (low)

- Where: `crates/verbatim-process/src/lib.rs:174-206` and `428-446`.
- Category: 7 (handle inheritance).
- The hole: the child ends of both pipes, and the log file, are created inheritable (`bInheritHandle: true`) before `CreateProcessW`, and stay inheritable until `launch` closes them. `PROC_THREAD_ATTRIBUTE_HANDLE_LIST` limits only this module's own launches. Any other `CreateProcess` in Core during that window with `bInheritHandles = TRUE` and no handle list would inherit them. `std::process::Command` is such a call, and so is third-party code. The leaked copy keeps a pipe open, so Core never sees end of stream from a dead child. Core spawns nothing today except through this module (I searched for `Command::new`, `CreateProcessW`, `ShellExecute`, and `wxExecute` across `verbatim-app`, `verbatim-gui`, and the outpost), so this is latent. The module comment's claim that a child inherits "never another child's handles" holds only while that remains true.
- Fix: create every handle non-inheritable, and make the three child-side handles inheritable only under a process-wide launch mutex. Inside the mutex, set the inherit flag, call `CreateProcessW`, and close them. Alternatively, keep them non-inheritable and duplicate them into the suspended child after creation, passing the duplicated values on the command line. Either way, state the residual limitation in the module comment.

#### C3. The proc-thread attribute list buffer is a `Vec<u8>` (low)

- Where: `crates/verbatim-process/src/lib.rs:273-274`.
- Category: 1 (raw buffers).
- The hole: the opaque `PROC_THREAD_ATTRIBUTE_LIST` holds pointer-sized fields, but `vec![0u8; size]` guarantees only 1-byte alignment. In practice the Windows system allocator returns 16-byte-aligned blocks, so nothing misbehaves today, but the SAFETY comment ("`buffer` is `size` bytes as the first call asked for") does not mention alignment, and alignment is not guaranteed by the type.
- Fix: allocate a `Vec<usize>` of `size.div_ceil(size_of::<usize>())` elements, as `server.rs:541` already does for `TOKEN_USER`, and say so in the comment.

#### C4. A job handle leaks when `SetInformationJobObject` fails (low)

- Where: `crates/verbatim-process/src/lib.rs:213-237`.
- Category: 7 (handles).
- The hole: the raw `job` handle is wrapped in `OwnedHandle` only at the end. If `SetInformationJobObject` fails, the `?` returns early and the handle leaks. No undefined behavior, but a resource leak for each failed launch.
- Fix: wrap the handle in `OwnedHandle` right after the invalid-handle check, and pass `as_raw_handle()` to `SetInformationJobObject`.

#### C5. Pid-based kill can hit a reused pid (low; test agent only)

- Where: `crates/verbatim-agent/src/process.rs:287-308` (`kill` for a pid the agent holds no handle for), and `kill_by_name` (`323-337`), which takes a snapshot and then opens pids.
- Category: 5 (handle and identifier reuse).
- The hole: between the caller learning a pid, or the snapshot, and `OpenProcess`, the process can exit and its pid be reused, so `TerminateProcess` ends an unrelated process. This also applies to a launched child once `status` has dropped its handle (`entry.child = None`), since `kill` then falls through to the open-by-pid path. The window is small, and the agent runs only on test machines.
- Fix: for children the agent launched, keep the handle until the entry expires (`FINISHED_KEPT`), not only until the first exit report, and never fall back to opening by pid while an entry exists. For `kill_by_name`, after `OpenProcess`, check that the image name of the opened handle still matches (`QueryFullProcessImageNameW`) before terminating.

#### C6. Cached top-level HWNDs are acted on after a delay (low; test agent only)

- Where: `crates/verbatim-agent/src/foreground.rs:251-276` (`ShowWindow`, `BringWindowToTop`, `SetForegroundWindow` on a window found by `main_window_of`) and `crates/verbatim-agent/src/desktop.rs:57-62` (`PostMessageW(WM_CLOSE)` to windows matched by title).
- Category: 5.
- The hole: the HWND is found by enumeration and used milliseconds later. If the window is destroyed in between and the handle value is reused, `SW_SHOW` or `WM_CLOSE` reaches an unrelated window. The SAFETY comments ("tolerates a stale handle", "tolerates a window that has since gone") are correct about memory safety but say nothing about acting on the wrong window.
- Fix: before acting, re-check that `GetWindowThreadProcessId` still returns one of the matched pids (for `foreground.rs`) and that the title still matches (for `desktop.rs`), and reword the comments to name the reuse risk. The window is milliseconds long, so this is hygiene rather than a live bug.

#### C7. mockapp leaks a SAFEARRAY when filling it fails (low)

- Where: `crates/mockapp/src/uia.rs:330-337` (`element_array_variant`), and the same pattern in `provider_array` (`432-440`) and `get_runtime_id` (`613-624`), which ignore put failures and return a partly filled array.
- Category: 1.
- The hole: on a `SafeArrayPutElement` failure, `element_array_variant` returns an empty variant without `SafeArrayDestroy`, which leaks the array. The other two return arrays with null `VT_UNKNOWN` slots, or zeroed `VT_I4` slots, to UIA. A null `VT_UNKNOWN` element is legal, so there is no undefined behavior, only a leak or wrong test data. Failure needs out-of-memory.
- Fix: call `SafeArrayDestroy(array)` on the failure path, and return an error rather than a partial array.

#### C8. mockapp's `accLocation` writes through out-pointers without a null check (low)

- Where: `crates/mockapp/src/msaa.rs:597-605`.
- Category: 1.
- The hole: the four `*mut i32` out-parameters are written unconditionally. Cross-process callers always reach this through the proxy and stub, which supply valid pointers. An in-process caller that passes null (a contract violation, but one COM servers usually guard against) would cause a null write: undefined behavior, in practice a crash. The SAFETY comment asserts something about the caller that the code does not check.
- Fix: return `E_POINTER` when any pointer is null, or write through `as_mut()` and skip null pointers.

#### C9. `inherited_pipes` trusts raw handle values from the command line (low)

- Where: `crates/verbatim-process/src/lib.rs:469-477`, called at `crates/verbatim-synth-host/src/main.rs:38` and `crates/verbatim-outpost/src/main.rs:52` and `64` (the outpost belongs to another part of this audit; listed here because it shares the contract).
- Category: 7.
- The hole: the `# Safety` contract, that each value is an inherited, owned, unshared handle, depends on argv, which nothing checks. Someone who starts `verbatim-synth-host` by hand with `--pipe-in 4` wraps whatever handle value 4 happens to be. Dropping the `File` then closes a handle some other component owns, a double close that can later close a reused handle: a handle use-after-close. Only a hand-run child is affected, not anything Core starts.
- Fix: before wrapping, validate each value with `GetFileType` equal to `FILE_TYPE_PIPE` and `GetHandleInformation` succeeding, so a bad value fails the start with a message. Keep the function `unsafe`, because ownership still cannot be proven, but make the callers' SAFETY comments say that the values are validated pipes Core passed.

### Wrong or unverifiable SAFETY comments (code fine or covered above)

- `crates/verbatim-process/src/lib.rs:275-276`: claims the buffer is suitable because it has the reported size; alignment is not addressed (C3).
- `crates/verbatim-process/src/lib.rs:115` ("a valid process handle we now own") is correct. `lib.rs:340` ("initialized above") is correct only because the `?` at 278-279 returns before reaching the delete; fine, but worth one more word.
- `crates/verbatim-synth-host/src/main.rs:36-37` and the outpost's two calls: "the values name the pipe ends Core created" is an assumption about argv that cannot be verified (C9).
- `crates/verbatim-agent/src/foreground.rs:253-254` and `crates/verbatim-agent/src/desktop.rs:58`: "tolerates a stale handle" is true for memory safety and silent on wrong-window action (C6).
- `crates/verbatim-agent/src/desktop.rs:65`: the comment "IsWindow tolerates any handle" sits on a statement whose closure holds the `unsafe` block, so it is attached to the wrong line for `clippy::undocumented_unsafe_blocks`. The claim itself is correct.
- `crates/mockapp/src/msaa.rs:597-599`: asserts every caller supplies valid out-pointers, which is not checked (C8).
- `crates/mockapp/src/window.rs:159-161`: "registering the same class name twice in one process is harmless" is not true in general: the second `RegisterClassExW` fails with `ERROR_CLASS_ALREADY_EXISTS`, which this function turns into an error. The comment goes on to say this happens only once, so the code is fine but the first clause is wrong.
- `crates/verbatim-control/src/server.rs:326-332` (`unsafe impl Send/Sync for RawPipe`): accurate as far as it goes, but it omits that `disconnect` (`DisconnectNamedPipe`) is also called from the writer thread and from `ControlServer::drop` on other threads while a read is pending. That is sound, since `DisconnectNamedPipe` is thread-safe and the pending operations complete with an error before their stack `OVERLAPPED` goes away. The comment should list it.
- `crates/verbatim-control/src/server.rs:376-386`: passes `lpNumberOfBytesRead` together with an `OVERLAPPED`, which the `ReadFile` documentation advises against ("use NULL ... to avoid potentially erroneous results"). The value is never used, since the count comes from `GetOverlappedResult`, so this is harmless. Passing `None`, as `tunnel.rs` does, would match the documentation. The same applies to `WriteFile` at 423-430.

### Unsafe blocks removable through existing safe wrappers

All of these are in mockapp's integration tests, which call the raw UIA interfaces that `verbatim-uia` now wraps publicly (`ElementExt`, `elements_of`, `Uia::property_condition`):

- `crates/mockapp/tests/events.rs:82-86`: the `Length` and `GetElement` loop becomes `verbatim_uia::elements_of(&children)`.
- `crates/mockapp/tests/uia_tree.rs:400-418`: same, `elements_of`.
- `crates/mockapp/tests/remote_ops.rs:58-66`: `uia.property_condition(...)` plus `ElementExt::find_first_build_cache`.
- `crates/mockapp/tests/controller_for.rs:36-43`: same as the previous item.
- `crates/mockapp/tests/remote_ops.rs:77`: `ElementExt::has_keyboard_focus`.
- `crates/mockapp/tests/remote_ops.rs:118`: `ElementExt::cached_value`.
- Not removable today: `events.rs:69` and `uia_tree.rs:390` (`FindAllBuildCache`, which has no wrapper), `remote_ops.rs:352-380` (transaction timeout, no wrapper), and all of `msaa_tree.rs` (`verbatim-ia2`'s `Accessible` wrapper is `pub(crate)`).

### Unsafe counts (lines containing the `unsafe` keyword, comment lines excluded)

- verbatim-agent: src 56, of which 4 are inside `#[cfg(test)]` modules (`tunnel.rs`); no `tests` directory. By file: `process.rs` 20, `tunnel.rs` 13, `desktop.rs` 12, `foreground.rs` 11.
- mockapp: src 32 (`window.rs` 20, `uia.rs` 8, `msaa.rs` 4); tests 31 (`msaa_tree.rs` 11, `remote_ops.rs` 6, `events.rs` 4, `common/mod.rs` 4, `uia_tree.rs` 3, `common/harness.rs` 1, `controller_for.rs` 1, `slow_application.rs` 1).
- verbatim-process: src 24 (`lib.rs` 17, `session.rs` 7); no tests.
- verbatim-control: src 28 (`server.rs` 27, including 3 `unsafe impl` lines; `send_keys.rs` 1); none in test modules.
- verbatim-synth-host: src 1 (`main.rs`); tests 1 (`hosting.rs`).

### Checked and found sound

- The control server's overlapped I/O: each stack `OVERLAPPED` is waited on with `GetOverlappedResult(bWait = TRUE)` before its frame returns, on every path including errors that are not pending. The `Arc<RawPipe>` keeps the handles alive until both threads finish. Each direction has its own manual-reset event. The `TOKEN_USER` buffer is 8-byte aligned. `LocalFree` is paired with both the SDDL and the SID string.
- The agent tunnel: on a stop it cancels with `CancelIoEx` and still waits for completion before the `OVERLAPPED` and the buffer go out of scope.
- `SendInput` callers (`send_keys.rs`, `foreground.rs`): `INPUT` is fully initialized and `cbSize` is `size_of::<INPUT>()`.
- mockapp: COM objects are created and served on the STA window thread, and `WM_GETOBJECT` arrives on that thread. No provider holds the tree mutex across a call that can pump messages or re-enter, since every lock is released before `provider_for`, `UiaHostProviderFromHwnd`, or `UiaRaise*`, so there is no same-thread re-lock. `SafeArrayPutElement` is given the interface pointer itself for `VT_UNKNOWN` and a pointer to the value for `VT_I4`, as documented. Returned `VARIANT`s and `BSTR`s transfer ownership through the `windows` crate's types. The `GWLP_USERDATA` context is leaked for the process's life and read as zero before setup.
- verbatim-process: the handle list and the job list attribute are both set before `CreateProcessW`; the attribute list is deleted on every path after initialization; both `hThread` and the parent's copies of the child-side handles are closed; the process handle moves into an `OwnedHandle`.
- The agent's TCP listener is unauthenticated and binds `0.0.0.0` by default. This is out of `unsafe` scope and already documented in `docs/crates/verbatim-agent.md` and `docs/tooling.md`, which run it on loopback locally. Noted only so it is not mistaken for an oversight: making loopback the default, with the VM passing its address explicitly, would make the safe choice the default.


## Part D: GUI, WASAPI, eSpeak NG, OneCore

Scope: `crates/verbatim-gui` (src, `cpp/gui.cpp`, `cpp/gui.h`, the cxx bridge in `src/bridge.rs`, `build.rs`), `crates/verbatim-audio-wasapi`, `crates/verbatim-synth-espeak` (src, tests, build.rs), `crates/verbatim-synth-onecore` (src, tests). Reviewed at c9114cb. wxWidgets behaviour was checked against the 3.3.3 source the build downloads (`target/debug/wxWidgets`, version.h confirms 3.3.3), eSpeak NG against the vendored `third_party/espeak-ng`, and cxx against the generated `bridge.rs.cc` and cxx's `unwind.rs`.

One medium finding (D1, raised from low during consolidation) and six low findings follow, then the checks that came out clean.

### Findings

#### D1. (medium) WASAPI write with zero frames passes a null pointer to copy_nonoverlapping

- Location: `crates/verbatim-audio-wasapi/src/lib.rs:431-463` (the `unsafe` block at 445).
- Category: 1 (raw pointers and buffers).
- Hole: `write` computes `frames = samples.len() / channels`. When `samples` is empty or shorter than one frame, `frame_count` is 0. Microsoft's `IAudioRenderClient::GetBuffer` documentation says that with `NumFramesRequested = 0` the method returns `S_OK` but does not write `*ppData`. The windows crate's wrapper zero-initializes the out value, so `buffer` is null, and `std::ptr::copy_nonoverlapping(samples.as_ptr(), null, 0)` follows. A null destination is undefined behaviour even for a count of 0. Debug builds, which CI and the end-to-end suite use, catch it as a precondition violation and abort the audio thread's process.
- Reachability: the mixer only calls `write` when `frames > 0` (`verbatim-audio/src/mixer.rs:799`), so today's caller never triggers it. But `AudioDevice::write` is a safe trait method, so any safe caller passing an empty slice gets undefined behaviour.
- Severity: medium (raised from the part D draft's low during consolidation): undefined behaviour reachable from a safe public trait method, even though no current caller reaches it.
- Fix: return `Ok(())` early when `frame_count == 0`, before `GetBuffer`.
- Checked: the GetBuffer documentation on learn.microsoft.com, the mixer's call site, and the standard library's `copy_nonoverlapping` preconditions.

#### D2. The WASAPI buffer alignment claim cannot be verified

- Location: `crates/verbatim-audio-wasapi/src/lib.rs:438-454`.
- Category: 1.
- Hole: the code casts the engine's `*mut u8` to `*mut f32` and copies typed. Both the `#[expect(clippy::cast_ptr_alignment)]` reason and the SAFETY comment say WASAPI buffers are aligned for their sample format. The GetBuffer documentation promises nothing about alignment. In practice packets start at whole-frame offsets of 4 times the channel count inside an engine allocation, so this is very likely fine, but the claim rests on undocumented behaviour, and a misaligned typed copy is undefined behaviour.
- Severity: low.
- Fix: copy bytes instead: `copy_nonoverlapping(samples.as_ptr().cast::<u8>(), buffer, frames * channels * 4)`. A byte copy has no alignment requirement, costs the same, and makes the `expect` unnecessary. Fold D1's early return into the same change.

#### D3. Bridge functions are declared safe but require the GUI thread, and `run_gui` can run twice

- Location: `crates/verbatim-gui/src/bridge.rs:288` (`unsafe extern "C++"`), `crates/verbatim-gui/src/lib.rs:134-163` (`run_gui`), `crates/verbatim-gui/cpp/gui.cpp:78-79` (`g_shell`).
- Category: 2 (threading) and 6 (cxx lifetimes).
- Hole: a function in an `unsafe extern "C++"` block without `unsafe fn` asserts that calling it from safe Rust is sound for every argument and in every context. Every function there except `wake_event_loop` reads the unsynchronized global `g_shell` and touches wxWidgets objects, so all of them are GUI-thread-only. Calling one from another thread (any safe code in the crate can) races on `g_shell` and drives wxWidgets off its thread: undefined behaviour. Today every call comes from a `GuiCore` method on the GUI thread, which I checked, so the rule holds only by crate-internal discipline.

  `run_gui` is a public safe function with no guard against a second concurrent or later call. Two threads calling it would race on `g_shell` and `g_wake_app` and run two wxApp instances, which is undefined behaviour. A second call after the first returns would re-run `wxEntryStart` after `wxEntryCleanup`, which wxWidgets does not support reliably. `verbatim-app` calls it once from main, so the risk is latent.
- Severity: low (latent; it is still a soundness hole in a safe public API).
- Fix: guard `run_gui` with a static `AtomicBool` that makes a second call return `GuiError`. Then either declare the GUI-thread-only bridge functions `unsafe fn`, or give each one a `&GuiCore` argument (which C++ ignores). `GuiCore` is `!Sync` because of its `RefCell`s, so a `&GuiCore` can only exist on the GUI thread and works as a proof of thread. Add a SAFETY comment on the extern block that states the thread rule (the block has none).

#### D4. A shell item click goes to a rectangle captured when the list was built

- Location: `crates/verbatim-gui/src/tray_list.rs:84-121` (`click`); the rectangles come from `src/shell_items.rs:280-293`.
- Category: 5 (a cached screen position acted on later; the rectangle counterpart of HWND reuse).
- Hole: the rectangles are read during enumeration, and the click happens whenever the user activates a button, possibly much later. In between, tray icons appear and disappear, the overflow flyout closes, and taskbar buttons reorder. `SetCursorPos` plus `SendInput` then left- or right-clicks whatever is at the old centre: a different tray icon or an unrelated window, with no check. NVDA's systrayList recipe has the same weakness. This is not memory-unsafe.
- Severity: low (a wrong-object action).
- Fix: when a button is activated, re-resolve the item by name on a worker thread (a fresh enumeration) and click only if one item still has that name. Or keep the element and use UIA Invoke or LegacyIAccessible's DoDefaultAction where available, falling back to the click.

#### D5. Rectangle arithmetic on values from a foreign provider can overflow

- Location: `crates/verbatim-gui/src/shell_items.rs:286-291` (`rect.right - rect.left`, `rect.bottom - rect.top`) and `:335-337` (`center_of`).
- Category: 1 (values trusted from other processes).
- Hole: the bounding rectangle comes from the shell's UIA provider in another process. Extreme values, for example `left = i32::MIN` with `right > 0`, make the subtraction overflow. Debug builds panic on the enumeration worker thread; the guard then sees the channel disconnect and shows nothing. Release builds wrap, and the result is filtered as a non-positive width. `center_of` can also overflow on `left + width / 2`. No undefined behaviour.
- Severity: low.
- Fix: compute in `i64`, or use `checked_sub` and drop the item on overflow.

#### D6. A long eSpeak NG data path silently falls back to the environment and the registry

- Location: `crates/verbatim-synth-espeak/src/lib.rs:273-305`, together with `third_party/espeak-ng/src/libespeak-ng/speech.c:247-339`.
- Category: 7.
- Hole: the driver checks that `<data>/phontab` exists through Rust's wide-character paths, then passes the path to `espeak_Initialize`. The comment says this path "must" hold the data, "so nothing else is ever loaded". eSpeak NG's `check_data_path` builds the path with `snprintf` into `path_home[N_PATH_BUF]`, where `N_PATH_BUF` is `_MAX_PATH` (260) on Windows. A data path whose UTF-8 length is near or above 260 bytes is truncated, the directory check fails, and `espeak_ng_InitializePath` falls back to the `ESPEAK_DATA_PATH` environment variable, then to `HKLM\Software\eSpeak NG\Path`. The synth host would then parse phoneme and dictionary data from another location, contradicting the comment. That data comes from a less trusted source, and eSpeak NG's data parser is not hardened against hostile input.

  The host's manifest sets the active code page to UTF-8 (`verbatim-synth-host.manifest`), so non-ASCII paths themselves are fine in the deployed host. Test binaries lack that manifest, so the same fallback happens there for a non-ASCII path.
- Severity: low (it needs an unusually deep install path, plus an environment or registry entry pointing elsewhere).
- Fix: before calling `espeak_Initialize`, reject a data path whose UTF-8 length plus `/espeak-ng-data` does not fit in 259 bytes. Optionally, after initializing, compare the data path eSpeak NG reports back (`espeak_Info`) with the one passed in.

#### D7. Rust panics in FFI callbacks abort the process instead of failing cleanly

- Locations: `crates/verbatim-synth-espeak/src/lib.rs:149-180` (`on_audio` calls `SynthSink::push_pcm`), and every `GuiCore` method called from C++ (`crates/verbatim-gui/src/lib.rs:190-448`).
- Category: 3 (panics crossing FFI).
- What happens: none of this is undefined behaviour. `on_audio` is `extern "C"`, and a panic out of it aborts (Rust 1.81 and later). Every `extern "Rust"` function of the bridge is wrapped in cxx's `prevent_unwind`, which aborts (checked in cxx `src/unwind.rs`). The C++ shims cxx generates are `noexcept` (checked in the generated `bridge.rs.cc`), so a C++ exception escaping `open_settings_dialog` and the like calls `std::terminate` rather than unwinding through Rust. The consequence is that any panic in GUI logic, including a `RefCell` double borrow, takes down all of Core. The same is true of the GUI's `on_ready` callback and of `SettingsHost` calls made from inside a callback. A panic in a sink takes down the synth host, which is isolated by design.
- Severity: low (informational; the borrow discipline was checked and holds today).
- Fix (optional): in `on_audio`, catch the panic with `catch_unwind`, set `stopped`, and return 1, so `speak` can report a `SynthError`. For the GUI, accept the abort, or wrap the bodies of the methods C++ calls in `catch_unwind` and log.

#### Note for the parent (outside part D's unsafe code)

The hidden-frame marker (`crates/verbatim-gui/src/hidden_frame.rs`, property name `verbatim_model::HIDDEN_FRAME_WINDOW_PROP`) is a public constant. Any process can `SetPropW` it on its own windows, and every outpost will then suppress them. It is low risk, since such an app could make itself inaccessible anyway, but the outposts trust the marker without checking that the window belongs to Core's process. A `GetWindowThreadProcessId` check against Core's PID on the outpost side would close it.

### Checked and found sound

- wxWidgets 3.3.3's `wxEvtHandler::CallAfter` is documented in `include/wx/event.h:3869` as "can be used from another thread" (it is `QueueEvent`). `wake_event_loop` reads `g_wake_app` under `g_wake_mutex`. `OnInit` publishes `g_wake_app` before calling `ready()`, and `OnExit` clears it under the same mutex before `wxEntryCleanup` deletes the app. A wake after the loop ends does nothing, and a send after `run_gui` returns fails because the receiver has been dropped.
- The `const GuiCore&` stored in `Shell` lives exactly as long as `run_event_loop`, whose Rust caller owns `core` on its stack. Queued `CallAfter` lambdas check `g_shell`, which stays valid until `wxEntryCleanup` has finished.
- Nested loops:
  - Pop-up menu: wxMSW's `WM_ENTERIDLE` handler (`src/msw/window.cpp:4318`) does not run idle processing, so no `drain` and no pending deletion happen inside `TrackPopupMenu`.
  - Modal Select Synthesizer dialog: `DoRunLoop` (`src/common/evtloopcmn.cpp:259-345`) checks `m_shouldExit` before `ProcessIdle`. After a `shut_down` that ends the modal loop, only pending events run, never `DeletePendingObjects`. The settings dialog, the parent of the dialog on the stack, therefore outlives `ChangeSynthesizer`.
  - Top-level `Destroy` is deferred (`src/common/toplvcmn.cpp:102-142`), so a `CloseDialog` reached from inside a dialog's own handler never frees `this` under that handler.
  - After shutdown, the lifecycle ignores every later request (`lifecycle.rs:91-169`), so no dialog is built with a null frame.
- No `RefCell` borrow is held across a call into C++: every `borrow_mut()` is a temporary dropped at the end of its statement. `list_button` clones the `Rc` callback out before running it, so `dialog_closed` taking `list_buttons` cannot free a running closure.
- `foreground.rs` and `hidden_frame.rs` get the HWNDs fresh from live wx objects on the GUI thread for every use, so no stale handle is cached. `shell_items` finds `Shell_TrayWnd` and its children fresh on each request.
- eSpeak NG:
  - Every event list carries the `user_data` given to `espeak_Synth`, including the terminators (`speech.c:226-231` and `465-487`), so `on_audio`'s dereference is valid.
  - `EspeakEvent`'s layout matches `espeak_EVENT`: 40 bytes, with `user_data` at offset 24 on both x64 and ARM64.
  - `wav` holds `count` samples, and the final call with a null `wav` is handled.
  - Text is checked for interior NUL; `size` includes the NUL.
  - `espeak_Initialize` with `DONT_EXIT` returns rather than exits on failure.
  - `IN_USE` enforces one instance per process; initialize and terminate are paired in `with_data`/`Drop`.
  - Every eSpeak NG call takes `&mut self` or happens during construction or drop. Since `EspeakSynth` is `Sync`, the `&self` methods that never call eSpeak NG are correct to stay that way.
- WASAPI:
  - The mix format is read through a packed struct by value and freed with `CoTaskMemFree` on every path before any early return.
  - `GetBuffer` is sized in frames and copies exactly `frames * channels` samples; a request larger than the free space fails with `AUDCLNT_E_BUFFER_TOO_LARGE`, not an overrun.
  - The device is opened, used, and dropped on the mixer's audio thread (`mixer.rs:245-300`), which made the MTA call to `CoInitializeEx`.
  - The notification callback holds its own `Arc`s and is unregistered before the enumerator is released.
  - `Event` closes its handle exactly once, and `Stream`'s field order releases the client before closing its event handle.
- OneCore: the single `CoInitializeEx` is never balanced, which is deliberate and harmless.

### Wrong or unverifiable SAFETY comments

- `crates/verbatim-audio-wasapi/src/lib.rs:442-444`: "the buffer is aligned for its format" is not stated anywhere in the API contract (D2), and the comment misses the zero-frame case (D1).
- `crates/verbatim-audio-wasapi/src/lib.rs:214-216`: the `Send` reason ("moved there before first use, and never shared") describes how the mixer uses the device today, but `Send` permits any later move. A more accurate reason: the MMDevice and audio client objects are free-threaded objects created in the MTA, so moving them to another thread is allowed, provided they are released on a thread that has COM initialized.
- `crates/verbatim-gui/src/bridge.rs:288`: the `unsafe extern "C++"` block has no SAFETY note, and its implicit claim that every function is safe to call anywhere is false for the GUI-thread-only functions (D3).
- `crates/verbatim-gui/src/shell_items.rs:183-184`: the doc comment says the base cache request "does not carry" the bounding rectangle. It does (`crates/verbatim-uia/src/cache.rs:83`), so the `AddProperty` call at line 189 is redundant.
- `crates/verbatim-gui/src/foreground.rs:92-93`: "a handle of this process's own frame" is imprecise: `focus_foreground` also passes dialog handles. The code is fine, since every handle is fresh and owned by this process.

### Unsafe blocks an existing safe wrapper could replace

All of these are in `crates/verbatim-gui/src/shell_items.rs`, using `verbatim_uia::{ElementExt, WalkerExt}`, which the crate already depends on:

- Line 189 (`cache.AddProperty(UIA_BoundingRectanglePropertyId)`): remove the block and `extended_cache_request` entirely and use `uia.base_cache_request()`, which already includes the property.
- Line 270 (`CachedNativeWindowHandle`): `element.cached_i32(UIA_NativeWindowHandlePropertyId)`, compared against the excluded HWND as an `isize`.
- Line 277 (`CachedControlType`): `element.cached_control_type()`.
- Lines 280-293 (`CachedName`, `CachedIsOffscreen`, `CachedBoundingRectangle`): `cached_string(UIA_NamePropertyId)`, `cached_bool(UIA_IsOffscreenPropertyId)`, and `cached_bounding_rectangle()`.
- Lines 307 and 316 (`GetFirstChildElementBuildCache`, `GetNextSiblingElementBuildCache`): `walker.first_child(element, cache)` and `walker.next_sibling(&current, cache)`, which also feed the crate's call counters.
- Line 142 (`ControlViewWalker`): no wrapper exists yet. `Uia::raw_view_walker` is the model, and a `control_view_walker` beside it would remove this block too.

That takes `shell_items.rs` from 10 unsafe sites to 3, all of them window lookups (`FindWindowW`, `FindWindowExW`, `IsWindowVisible`). `verbatim-ia2/src/window.rs` has safe equivalents, but they are `pub(crate)`.

### Unsafe counts

Code lines containing `unsafe`, with comment lines excluded:

- `verbatim-gui`: src 17, made up of `shell_items.rs` 10 (counting the D3 removals above), `foreground.rs` 2, `hidden_frame.rs` 2, `tray_list.rs` 2, and `bridge.rs` 1 (the `unsafe extern "C++"` block). `build.rs` and `build/` have 0, and there is no tests directory. The C++ in `cpp/gui.cpp` is about 800 lines, all of it implicitly unsafe and reviewed above.
- `verbatim-audio-wasapi`: src 18, including 3 `unsafe impl` (`Send` and `Sync` for `Event`, `Send` for `WasapiDevice`). There is no tests directory.
- `verbatim-synth-espeak`: src 14, made up of the `unsafe extern "C"` block, the callback type, `unsafe extern "C" fn on_audio`, `unsafe fn read_c`, and 10 blocks. Tests and `build.rs` have 0.
- `verbatim-synth-onecore`: src 1 (`CoInitializeEx`); tests 0.
